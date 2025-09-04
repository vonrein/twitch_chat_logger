use crate::config::ClientConfig;
use crate::connection::ConnectionIncomingMessage;
use crate::error::Error;
use crate::irc;
use crate::login::{CredentialsPair, LoginCredentials};
use crate::message::commands::ServerMessage;
use crate::message::AsRawIRC;
use crate::message::IRCMessage;
#[cfg(feature = "metrics-collection")]
use crate::metrics::MetricsBundle;
use crate::transport::Transport;
use either::Either;
use enum_dispatch::enum_dispatch;
use futures_util::{SinkExt, StreamExt};
use std::collections::VecDeque;
use std::convert::TryFrom;
use std::sync::{Arc, Weak};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval_at, Duration, Instant};
use tracing::{debug_span, info_span, Instrument};

#[derive(Debug)]
pub(crate) enum ConnectionLoopCommand<T: Transport, L: LoginCredentials> {
    // commands that come from Connection methods
    SendMessage(IRCMessage, Option<oneshot::Sender<Result<(), Error<T, L>>>>),

    // comes from the init task
    TransportInitFinished(Result<(T, CredentialsPair), Error<T, L>>),

    // comes from the task(s) spawned when a message is sent
    SendError(Arc<T::OutgoingError>),

    // commands that come from the incoming loop
    // Some(Ok(_)) is an ordinary message, Some(Err(_)) an error, and None an EOF (end of stream)
    IncomingMessage(Option<Result<IRCMessage, Error<T, L>>>),

    // commands that come from the ping loop
    SendPing(),
    CheckPong(),
}

#[enum_dispatch]
trait ConnectionLoopStateMethods<T: Transport, L: LoginCredentials> {
    fn send_message(
        &mut self,
        message: IRCMessage,
        reply_sender: Option<oneshot::Sender<Result<(), Error<T, L>>>>,
    );
    fn on_transport_init_finished(
        self,
        init_result: Result<(T, CredentialsPair), Error<T, L>>,
    ) -> ConnectionLoopState<T, L>;
    fn on_send_error(self, error: Arc<T::OutgoingError>) -> ConnectionLoopState<T, L>;
    fn on_incoming_message(
        self,
        maybe_message: Option<Result<IRCMessage, Error<T, L>>>,
    ) -> ConnectionLoopState<T, L>;
    fn send_ping(&mut self);
    fn check_pong(self) -> ConnectionLoopState<T, L>;
}

#[enum_dispatch(ConnectionLoopStateMethods < T, L >)]
enum ConnectionLoopState<T: Transport, L: LoginCredentials> {
    Initializing(ConnectionLoopInitializingState<T, L>),
    Open(ConnectionLoopOpenState<T, L>),
    Closed(ConnectionLoopClosedState<T, L>),
}

pub(crate) struct ConnectionLoopWorker<T: Transport, L: LoginCredentials> {
    connection_loop_rx: mpsc::UnboundedReceiver<ConnectionLoopCommand<T, L>>,
    state: ConnectionLoopState<T, L>,
    #[cfg(feature = "metrics-collection")]
    metrics: Option<MetricsBundle>,
}

impl<T: Transport, L: LoginCredentials> ConnectionLoopWorker<T, L> {
    pub fn spawn(
        config: Arc<ClientConfig<L>>,
        connection_incoming_tx: mpsc::UnboundedSender<ConnectionIncomingMessage<T, L>>,
        connection_loop_tx: Weak<mpsc::UnboundedSender<ConnectionLoopCommand<T, L>>>,
        connection_loop_rx: mpsc::UnboundedReceiver<ConnectionLoopCommand<T, L>>,
        connection_id: usize,
        #[cfg(feature = "metrics-collection")] metrics: Option<MetricsBundle>,
    ) {
        let worker = ConnectionLoopWorker {
            connection_loop_rx,
            state: ConnectionLoopState::Initializing(ConnectionLoopInitializingState {
                commands_queue: VecDeque::new(),
                connection_loop_tx: Weak::clone(&connection_loop_tx),
                connection_incoming_tx,
                #[cfg(feature = "metrics-collection")]
                metrics: metrics.clone(),
            }),
            #[cfg(feature = "metrics-collection")]
            metrics,
        };

        let main_connection_span = info_span!("connection", id = connection_id);
        let _enter = main_connection_span.enter();
        tokio::spawn(
            ConnectionLoopWorker::run_init_task(config, connection_loop_tx)
                .instrument(info_span!("init_task")),
        );
        tokio::spawn(worker.run().instrument(info_span!("main_loop")));
    }

    async fn run_init_task(
        config: Arc<ClientConfig<L>>,
        connection_loop_tx: Weak<mpsc::UnboundedSender<ConnectionLoopCommand<T, L>>>,
    ) {
        tracing::debug!("Spawned connection init task");
        // async{}.await is used in place of a try block since they are not stabilized yet
        // TODO revise this once try blocks are stabilized
        let res = async {
            let credentials = config
                .login_credentials
                .get_credentials()
                .await
                .map_err(Arc::new)
                .map_err(Error::LoginError)?;

            // rate limits the opening of new connections
            tracing::trace!("Trying to acquire permit for opening transport...");
            let rate_limit_permit = Arc::clone(&config.connection_rate_limiter)
                .acquire_owned()
                .await;
            tracing::trace!("Successfully got permit to open transport.");

            let connect_attempt = T::new();
            let timeout = tokio::time::sleep(config.connect_timeout);

            let transport = tokio::select! {
                t_result = connect_attempt => {
                    t_result.map_err(Arc::new)
                        .map_err(Error::ConnectError)
                },
                _ = timeout => {
                    Err(Error::ConnectTimeout)
                }
            }?;

            // release the rate limit permit after the transport is connected and after
            // the specified time has elapsed.
            tokio::spawn(
                async move {
                    tokio::time::sleep(config.new_connection_every).await;
                    drop(rate_limit_permit);
                    tracing::trace!(
                        "Successfully released permit after waiting specified duration."
                    );
                }
                .instrument(debug_span!("release_permit_task")),
            );

            Ok::<(T, CredentialsPair), Error<T, L>>((transport, credentials))
        }
        .await;

        // res is now the result of the init work
        if let Some(connection_loop_tx) = connection_loop_tx.upgrade() {
            connection_loop_tx
                .send(ConnectionLoopCommand::TransportInitFinished(res))
                .ok();
        }
    }

    async fn run(mut self) {
        tracing::debug!("Spawned connection event loop");
        while let Some(command) = self.connection_loop_rx.recv().await {
            self = self.process_command(command);
        }
        tracing::debug!("Connection event loop ended")
    }

    /// Process a command, consuming the current state and returning a new state
    fn process_command(mut self, command: ConnectionLoopCommand<T, L>) -> Self {
        match command {
            ConnectionLoopCommand::SendMessage(message, reply_sender) => {
                self.state.send_message(message, reply_sender);
            }
            ConnectionLoopCommand::TransportInitFinished(init_result) => {
                self.state = self.state.on_transport_init_finished(init_result);
            }
            ConnectionLoopCommand::SendError(error) => {
                self.state = self.state.on_send_error(error);
            }
            ConnectionLoopCommand::IncomingMessage(maybe_msg) => {
                match &maybe_msg {
                    Some(Ok(msg)) => {
                        tracing::trace!("< {}", msg.as_raw_irc());
                        #[cfg(feature = "metrics-collection")]
                        if let Some(ref metrics) = self.metrics {
                            metrics
                                .messages_received
                                .with_label_values(&[&msg.command])
                                .inc();
                        }
                    }
                    Some(Err(e)) => tracing::trace!("Error from transport: {}", e),
                    None => tracing::trace!("EOF from transport"),
                }

                self.state = self.state.on_incoming_message(maybe_msg);
            }
            ConnectionLoopCommand::SendPing() => self.state.send_ping(),
            ConnectionLoopCommand::CheckPong() => {
                self.state = self.state.check_pong();
            }
        };
        self
    }
}

type CommandQueue<T, L> = VecDeque<(IRCMessage, Option<oneshot::Sender<Result<(), Error<T, L>>>>)>;
type MessageReceiver<T, L> =
    mpsc::UnboundedReceiver<(IRCMessage, Option<oneshot::Sender<Result<(), Error<T, L>>>>)>;
type MessageSender<T, L> =
    mpsc::UnboundedSender<(IRCMessage, Option<oneshot::Sender<Result<(), Error<T, L>>>>)>;

//
// INITIALIZING STATE
//
struct ConnectionLoopInitializingState<T: Transport, L: LoginCredentials> {
    // a list of queued up ConnectionLoopCommand::SendMessage messages
    commands_queue: CommandQueue<T, L>,
    connection_loop_tx: Weak<mpsc::UnboundedSender<ConnectionLoopCommand<T, L>>>,
    connection_incoming_tx: mpsc::UnboundedSender<ConnectionIncomingMessage<T, L>>,
    #[cfg(feature = "metrics-collection")]
    metrics: Option<MetricsBundle>,
}

impl<T: Transport, L: LoginCredentials> ConnectionLoopInitializingState<T, L> {
    fn transition_to_closed(self, err: Error<T, L>) -> ConnectionLoopState<T, L> {
        tracing::info!("Closing connection, reason: {}", err);

        for (_message, return_sender) in self.commands_queue.into_iter() {
            if let Some(return_sender) = return_sender {
                return_sender.send(Err(err.clone())).ok();
            }
        }

        self.connection_incoming_tx
            .send(ConnectionIncomingMessage::StateClosed { cause: err.clone() })
            .ok();

        // return the new state the connection should take on
        ConnectionLoopState::Closed(ConnectionLoopClosedState {
            reason_for_closure: err,
        })
    }

    async fn run_incoming_forward_task(
        mut transport_incoming: T::Incoming,
        connection_loop_tx: Weak<mpsc::UnboundedSender<ConnectionLoopCommand<T, L>>>,
        mut shutdown_notify: oneshot::Receiver<()>,
    ) {
        tracing::debug!("Spawned incoming messages forwarder");
        loop {
            tokio::select! {
                _ = &mut shutdown_notify => {
                    // got kill signal
                    break;
                }
                incoming_message = transport_incoming.next() => {
                    let do_exit = matches!(incoming_message, None | Some(Err(_)));
                    let incoming_message = incoming_message.map(|x| x.map_err(|e| match e {
                        Either::Left(e) => Error::IncomingError(Arc::new(e)),
                        Either::Right(e) => Error::IRCParseError(e)
                    }));

                    if let Some(connection_loop_tx) = connection_loop_tx.upgrade() {
                        connection_loop_tx.send(ConnectionLoopCommand::IncomingMessage(incoming_message)).ok();
                    } else {
                        break;
                    }

                    if do_exit {
                        break;
                    }
                }
            }
        }
        tracing::debug!("Incoming messages forwarder ended");
    }

    async fn run_outgoing_forward_task(
        mut transport_outgoing: T::Outgoing,
        mut messages_rx: MessageReceiver<T, L>,
        connection_loop_tx: Weak<mpsc::UnboundedSender<ConnectionLoopCommand<T, L>>>,
    ) {
        tracing::debug!("Spawned outgoing messages forwarder");
        while let Some((message, reply_sender)) = messages_rx.recv().await {
            let res = transport_outgoing.send(message).await.map_err(Arc::new);

            // The error is cloned and sent both to the calling method as well as
            // the connection event loop so it can end with that error.
            if let Err(ref err) = res {
                if let Some(connection_loop_tx) = connection_loop_tx.upgrade() {
                    connection_loop_tx
                        .send(ConnectionLoopCommand::SendError(Arc::clone(err)))
                        .ok();
                }
            }

            if let Some(reply_sender) = reply_sender {
                reply_sender.send(res.map_err(Error::OutgoingError)).ok();
            }
        }
    }

    async fn run_ping_task(
        connection_loop_tx: Weak<mpsc::UnboundedSender<ConnectionLoopCommand<T, L>>>,
        mut shutdown_notify: oneshot::Receiver<()>,
    ) {
        tracing::debug!("Spawned pinger task");
        // every 30 seconds we send out a PING
        // 5 seconds after sending it out, we check that we got a PONG message since sending that PING
        // if not, the connection is failed with an error (Error::PingTimeout)
        let ping_every = Duration::from_secs(30);
        let check_pong_after = Duration::from_secs(5);

        let mut send_ping_interval = interval_at(Instant::now() + ping_every, ping_every);
        let mut check_pong_interval =
            interval_at(Instant::now() + ping_every + check_pong_after, ping_every);

        loop {
            tokio::select! {
                _ = &mut shutdown_notify => {
                    break;
                },
                _ = send_ping_interval.tick() => {
                    tracing::trace!("sending ping");
                    if let Some(connection_loop_tx) = connection_loop_tx.upgrade() {
                        connection_loop_tx.send(ConnectionLoopCommand::SendPing()).ok();
                    } else {
                        break;
                    }
                }
                _ = check_pong_interval.tick() => {
                    tracing::trace!("checking for pong");
                    if let Some(connection_loop_tx) = connection_loop_tx.upgrade() {
                        connection_loop_tx.send(ConnectionLoopCommand::CheckPong()).ok();
                    } else {
                        break;
                    }
                }
            }
        }
        tracing::debug!("Pinger task ended");
    }
}

impl<T: Transport, L: LoginCredentials> ConnectionLoopStateMethods<T, L>
    for ConnectionLoopInitializingState<T, L>
{
    fn send_message(
        &mut self,
        message: IRCMessage,
        reply_sender: Option<oneshot::Sender<Result<(), Error<T, L>>>>,
    ) {
        self.commands_queue.push_back((message, reply_sender));
    }

    fn on_transport_init_finished(
        self,
        init_result: Result<(T, CredentialsPair), Error<T, L>>,
    ) -> ConnectionLoopState<T, L> {
        match init_result {
            Ok((transport, credentials)) => {
                // transport was opened successfully
                tracing::debug!("Transport init task has finished, transitioning to Initializing");
                let (transport_incoming, transport_outgoing) = transport.split();

                let (kill_incoming_loop_tx, kill_incoming_loop_rx) = oneshot::channel();
                tokio::spawn(
                    ConnectionLoopInitializingState::run_incoming_forward_task(
                        transport_incoming,
                        Weak::clone(&self.connection_loop_tx),
                        kill_incoming_loop_rx,
                    )
                    .instrument(info_span!("incoming_forward_task")),
                );

                let (outgoing_messages_tx, outgoing_messages_rx) = mpsc::unbounded_channel();
                tokio::spawn(
                    ConnectionLoopInitializingState::run_outgoing_forward_task(
                        transport_outgoing,
                        outgoing_messages_rx,
                        Weak::clone(&self.connection_loop_tx),
                    )
                    .instrument(info_span!("outgoing_forward_task")),
                );

                let (kill_pinger_tx, kill_pinger_rx) = oneshot::channel();
                tokio::spawn(
                    ConnectionLoopInitializingState::run_ping_task(
                        Weak::clone(&self.connection_loop_tx),
                        kill_pinger_rx,
                    )
                    .instrument(info_span!("ping_task")),
                );

                // transition our own state from Initializing to Open
                #[cfg(feature = "metrics-collection")]
                self.connection_incoming_tx
                    .send(ConnectionIncomingMessage::StateOpen)
                    .ok();

                let mut new_state = ConnectionLoopState::Open(ConnectionLoopOpenState {
                    connection_incoming_tx: self.connection_incoming_tx,
                    outgoing_messages_tx,
                    pong_received: false,
                    kill_incoming_loop_tx: Some(kill_incoming_loop_tx),
                    kill_pinger_tx: Some(kill_pinger_tx),
                    #[cfg(feature = "metrics-collection")]
                    metrics: self.metrics,
                });

                new_state.send_message(
                    irc!["CAP", "REQ", "twitch.tv/tags twitch.tv/commands"],
                    None,
                );
                if let Some(token) = credentials.token {
                    new_state.send_message(irc!["PASS", format!("oauth:{}", token)], None);
                }
                new_state.send_message(irc!["NICK", credentials.login], None);

                new_state.send_message(irc!["CAP", "REQ", "twitch.tv/membership"], None);

                for (message, return_sender) in self.commands_queue.into_iter() {
                    new_state.send_message(message, return_sender);
                }

                new_state
            }
            Err(init_error) => {
                // emit error to downstream + transition to closed
                tracing::error!("Transport init task has finished with error, closing connection");
                self.transition_to_closed(init_error)
            }
        }
    }

    fn on_send_error(self, error: Arc<T::OutgoingError>) -> ConnectionLoopState<T, L> {
        self.transition_to_closed(Error::OutgoingError(error))
    }

    fn on_incoming_message(
        self,
        _maybe_message: Option<Result<IRCMessage, Error<T, L>>>,
    ) -> ConnectionLoopState<T, L> {
        unreachable!("messages cannot come in while initializing")
    }

    fn send_ping(&mut self) {
        unreachable!("pinger should not run while initializing")
    }

    fn check_pong(self) -> ConnectionLoopState<T, L> {
        unreachable!("pinger should not run while initializing")
    }
}

//
// OPEN STATE
//
struct ConnectionLoopOpenState<T: Transport, L: LoginCredentials> {
    connection_incoming_tx: mpsc::UnboundedSender<ConnectionIncomingMessage<T, L>>,
    outgoing_messages_tx: MessageSender<T, L>,
    pong_received: bool,
    /// To kill the background pinger and forward tasks when this gets dropped.
    /// These fields are wrapped in `Option` so we can use `take()` in the Drop implementation.
    kill_incoming_loop_tx: Option<oneshot::Sender<()>>,
    kill_pinger_tx: Option<oneshot::Sender<()>>,
    #[cfg(feature = "metrics-collection")]
    metrics: Option<MetricsBundle>,
}

impl<T: Transport, L: LoginCredentials> ConnectionLoopOpenState<T, L> {
    fn transition_to_closed(self, cause: Error<T, L>) -> ConnectionLoopState<T, L> {
        tracing::info!("Closing connection, cause: {}", cause);

        self.connection_incoming_tx
            .send(ConnectionIncomingMessage::StateClosed {
                cause: cause.clone(),
            })
            .ok();

        // the shutdown notify is invoked via the Drop implementation

        // return the new state the connection should take on
        ConnectionLoopState::Closed(ConnectionLoopClosedState {
            reason_for_closure: cause,
        })
    }
}

impl<T: Transport, L: LoginCredentials> Drop for ConnectionLoopOpenState<T, L> {
    fn drop(&mut self) {
        self.kill_incoming_loop_tx.take().unwrap().send(()).ok();
        self.kill_pinger_tx.take().unwrap().send(()).ok();
    }
}

impl<T: Transport, L: LoginCredentials> ConnectionLoopStateMethods<T, L>
    for ConnectionLoopOpenState<T, L>
{
    fn send_message(
        &mut self,
        message: IRCMessage,
        reply_sender: Option<oneshot::Sender<Result<(), Error<T, L>>>>,
    ) {
        tracing::trace!("> {}", message.as_raw_irc());
        #[cfg(feature = "metrics-collection")]
        if let Some(ref metrics) = self.metrics {
            metrics
                .messages_sent
                .with_label_values(&[&message.command])
                .inc();
        }

        self.outgoing_messages_tx.send((message, reply_sender)).ok();
    }

    fn on_transport_init_finished(
        self,
        _init_result: Result<(T, CredentialsPair), Error<T, L>>,
    ) -> ConnectionLoopState<T, L> {
        unreachable!("transport init cannot finish more than once")
    }

    fn on_send_error(self, error: Arc<T::OutgoingError>) -> ConnectionLoopState<T, L> {
        self.transition_to_closed(Error::OutgoingError(error))
    }

    fn on_incoming_message(
        mut self,
        maybe_message: Option<Result<IRCMessage, Error<T, L>>>,
    ) -> ConnectionLoopState<T, L> {
        match maybe_message {
            None => {
                tracing::info!("EOF received from transport incoming stream");
                self.transition_to_closed(Error::RemoteUnexpectedlyClosedConnection)
            }
            Some(Err(error)) => {
                tracing::error!("Error received from transport incoming stream: {}", error);
                self.transition_to_closed(error)
            }
            Some(Ok(irc_message)) => {
                // Note! An error here (failing to parse to a ServerMessage) will not result
                // in a connection abort. This is by design. See for example
                // https://github.com/robotty/dank-twitch-irc/issues/22.
                // The message will just be ignored instead
                let server_message = ServerMessage::try_from(irc_message);

                match server_message {
                    Ok(server_message) => {
                        self.connection_incoming_tx
                            .send(ConnectionIncomingMessage::IncomingMessage(Box::new(
                                server_message.clone(),
                            )))
                            .ok();

                        // handle message
                        // react to PING, PONG and RECONNECT
                        match &server_message {
                            ServerMessage::Ping(_) => {
                                self.send_message(irc!["PONG", "tmi.twitch.tv"], None);
                            }
                            ServerMessage::Pong(_) => {
                                tracing::trace!("Received pong");
                                self.pong_received = true;
                            }
                            ServerMessage::Reconnect(_) => {
                                // disconnect
                                return self.transition_to_closed(Error::ReconnectCmd);
                            }
                            _ => {}
                        }
                    }
                    Err(parse_error) => {
                        tracing::error!("Failed to parse incoming message as ServerMessage (emitting as generic instead): {}", parse_error);
                        self.connection_incoming_tx
                            .send(ConnectionIncomingMessage::IncomingMessage(Box::new(
                                ServerMessage::new_generic(IRCMessage::from(parse_error)),
                            )))
                            .ok();
                    }
                }

                // stay open
                ConnectionLoopState::Open(self)
            }
        }
    }

    fn send_ping(&mut self) {
        self.pong_received = false;
        self.send_message(irc!["PING", "tmi.twitch.tv"], None);
    }

    fn check_pong(self) -> ConnectionLoopState<T, L> {
        if !self.pong_received {
            // close down
            self.transition_to_closed(Error::PingTimeout)
        } else {
            // stay open
            ConnectionLoopState::Open(self)
        }
    }
}

//
// CLOSED STATE.
//
struct ConnectionLoopClosedState<T: Transport, L: LoginCredentials> {
    reason_for_closure: Error<T, L>,
}

impl<T: Transport, L: LoginCredentials> ConnectionLoopStateMethods<T, L>
    for ConnectionLoopClosedState<T, L>
{
    fn send_message(
        &mut self,
        _message: IRCMessage,
        reply_sender: Option<oneshot::Sender<Result<(), Error<T, L>>>>,
    ) {
        if let Some(reply_sender) = reply_sender {
            reply_sender.send(Err(self.reason_for_closure.clone())).ok();
        }
    }

    fn on_transport_init_finished(
        self,
        _init_result: Result<(T, CredentialsPair), Error<T, L>>,
    ) -> ConnectionLoopState<T, L> {
        // do nothing, stay closed
        ConnectionLoopState::Closed(self)
    }

    fn on_send_error(self, _error: Arc<T::OutgoingError>) -> ConnectionLoopState<T, L> {
        // do nothing, stay closed
        ConnectionLoopState::Closed(self)
    }

    fn on_incoming_message(
        self,
        _maybe_message: Option<Result<IRCMessage, Error<T, L>>>,
    ) -> ConnectionLoopState<T, L> {
        // do nothing, stay closed
        ConnectionLoopState::Closed(self)
    }

    fn send_ping(&mut self) {}

    fn check_pong(self) -> ConnectionLoopState<T, L> {
        // do nothing, stay closed
        ConnectionLoopState::Closed(self)
    }
}

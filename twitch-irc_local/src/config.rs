use crate::login::{LoginCredentials, StaticLoginCredentials};
use std::borrow::Cow;
#[cfg(feature = "metrics-collection")]
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Configures settings for a `TwitchIRCClient`.
#[derive(Debug)]
pub struct ClientConfig<L: LoginCredentials> {
    /// Gets a set of credentials every time the client needs to log in on a new connection.
    /// See [`LoginCredentials`] for details.
    pub login_credentials: L,

    /// A new connection will automatically be created if a channel is joined and all
    /// currently established connections have joined at least this many channels.
    pub max_channels_per_connection: usize,

    /// A new connection will automatically be created if any message is to be sent
    /// and all currently established connections have recently sent more than this many
    /// messages (time interval is defined by `max_waiting_messages_duration_window`)
    pub max_waiting_messages_per_connection: usize,

    /// We assume messages to be "waiting" for this amount of time after sending them out, e.g.
    /// typically 100 or 150 milliseconds (purely a value that has been measured/observed,
    /// not documented or fixed in any way)
    pub time_per_message: Duration,

    /// rate-limits the opening of new connections. By default this is constructed with 1 permit
    /// only, which means connections cannot be opened in parallel. If this is set to more than 1
    /// permit, then that many connections can be opened in parallel.
    ///
    /// This is designed to be wrapped in an Arc to allow it to be shared between multiple
    /// TwitchIRCClient instances.
    pub connection_rate_limiter: Arc<Semaphore>,

    /// Allow a new connection to be made after this period has elapsed. By default this is set
    /// to 2 seconds, and combined with the permits=1 of the semaphore, allows one connection
    /// to be made every 2 seconds.
    ///
    /// More specifically, after taking the permit from the semaphore, the permit will be put
    /// back after this period has elapsed.
    pub new_connection_every: Duration,

    /// Imposes a general timeout for new connections. This is in place in addition to possible
    /// operating system timeouts (E.g. for new TCP connections), since additional "connect" work
    /// takes place after the TCP connection is opened, e.g. to set up TLS or perform a WebSocket
    /// handshake. Default value: 20 seconds.
    pub connect_timeout: Duration,

    /// Disable or enable and configure the collection of metrics on this `TwitchIRCClient`
    /// using the `prometheus` crate. See more information about the possible options on the
    /// [`MetricsConfig`] enum.
    ///
    /// This crate is currently capable of exporting the following prometheus metrics:
    /// * `twitchirc_messages_received` with label `command` counts all incoming messages. (Counter)
    ///
    /// * `twitchirc_messages_sent` counts messages sent out, with a `command` label. (Counter)
    ///
    /// * `twitchirc_channels` with `type=allocated/confirmed` counts how many channels
    ///   you are joined to (Gauge). Allocated channels are joins that passed through the `TwitchIRCClient`
    ///   but may be waiting e.g. for the connection to finish connecting. Once a
    ///   confirmation response is received by Twitch that the channel was joined successfully,
    ///   that channel is additionally `confirmed`.
    ///
    /// * `twitchirc_connections` counts how many connections this client has in use (Gauge).
    ///    The label `state=initializing/open` identifies how many connections are
    ///    in the process of connecting (`initializing`) vs how many connections are already established (`open`).
    ///
    /// * `twitchirc_connections_failed` counts every time a connection fails (Counter). Note however, depending
    ///   on conditions e.g. how many channels were joined on that channel, there can be cases where
    ///   a connection failing would not mandate the creation of a new connection (e.g. if
    ///   you have parted channels on other connections, making it so all the channels the failed
    ///   connection was joined to can be re-joined on those already existing connections).
    ///
    /// * `twitchirc_connections_created` on the other hand tracks how many times, since
    ///   the creation of the client, a new connection has been made.
    #[cfg(feature = "metrics-collection")]
    pub metrics_config: MetricsConfig,

    /// Allows you to differentiate between multiple clients with
    /// [the `tracing` crate](https://docs.rs/tracing).
    ///
    /// This library logs a variety of trace, debug, info, warning and error messages using the
    /// `tracing` crate. An example log line using the default `tracing_subscriber` output format
    /// might look like this:
    ///
    /// ```text
    /// 2022-02-07T10:44:23.297571Z  INFO client_loop: twitch_irc::client::event_loop: Making a new pool connection, new ID is 0
    /// ```
    ///
    /// You may optionally set this configuration variable to some string, which will then
    /// modify all log messages by giving the `client_loop` span the `name` attribute:
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use twitch_irc::ClientConfig;
    ///
    /// let mut config = ClientConfig::default();
    /// config.tracing_identifier = Some(Cow::Borrowed("bot_one"));
    /// ```
    ///
    /// All log output will then look like this (note the additional `{name=bot_one}`:
    ///
    /// ```text
    /// 2022-02-07T10:48:34.769272Z  INFO client_loop{name=bot_one}: twitch_irc::client::event_loop: Making a new pool connection, new ID is 0
    /// ```
    ///
    /// Essentially, this library makes use of `tracing` `Span`s to differentiate between
    /// different async tasks and to also differentiate log messages coming from different
    /// connections.
    ///
    /// Specifying this option will further allow you to differentiate between multiple
    /// clients if your application is running multiple of them. It will add the `name=your_value`
    /// attribute to the `client_loop` span, which is root for all further deeper spans in the
    /// client. This means that all log output from a single client will all be under that span,
    /// with that name.
    pub tracing_identifier: Option<Cow<'static, str>>,
}

/// Used to configure the options around metrics collection using the `prometheus` crate.
#[cfg(feature = "metrics-collection")]
#[derive(Debug)]
pub enum MetricsConfig {
    /// Metrics are not collected. Metrics are not registered with any registry.
    ///
    /// Useful if an application only requires monitoring on some of its running `twitch-irc`
    /// clients.
    Disabled,
    /// Metrics are collected. The metrics are immediately registered when
    /// [`TwitchIRCClient::new`](crate::TwitchIRCClient::new) is called.
    Enabled {
        /// Add these "constant labels" to all metrics produced by this client. This allows you
        /// to, for example, differentiate between multiple clients by naming them, or you
        /// may wish to place other relevant metadata pertaining to the whole client on all the
        /// metrics.
        ///
        /// This defaults to an empty map.
        constant_labels: HashMap<String, String>,
        /// Specifies what [`Registry`](prometheus::Registry) to register all metrics for this
        /// client with.
        ///
        /// Defaults to `None`, in which case the metrics are registered with the
        /// [global default registry of the `prometheus` crate](prometheus::default_registry).
        metrics_registry: Option<prometheus::Registry>,
    },
}

#[cfg(feature = "metrics-collection")]
impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig::Enabled {
            constant_labels: HashMap::new(),
            metrics_registry: None,
        }
    }
}

impl<L: LoginCredentials> ClientConfig<L> {
    /// Create a new configuration from the given login credentials, with all other configuration
    /// options being default.
    pub fn new_simple(login_credentials: L) -> ClientConfig<L> {
        ClientConfig {
            login_credentials,
            max_channels_per_connection: 90,

            max_waiting_messages_per_connection: 5,
            time_per_message: Duration::from_millis(150),

            // 1 connection every 2 seconds seems to work well
            connection_rate_limiter: Arc::new(Semaphore::new(1)),
            new_connection_every: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(20),

            #[cfg(feature = "metrics-collection")]
            metrics_config: MetricsConfig::default(),
            tracing_identifier: None,
        }
    }
}

impl Default for ClientConfig<StaticLoginCredentials> {
    fn default() -> ClientConfig<StaticLoginCredentials> {
        ClientConfig::new_simple(StaticLoginCredentials::anonymous())
    }
}

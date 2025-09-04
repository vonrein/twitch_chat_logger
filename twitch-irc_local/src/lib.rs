#![warn(missing_docs)]
//! This is a client library to interface with [Twitch](https://www.twitch.tv/) chat.
//!
//! This library is async and runs using the `tokio` runtime.
//!
//! # Getting started
//!
//! The central feature of this library is the `TwitchIRCClient` which connects to Twitch IRC
//! for you using a pool of connections and handles all the important bits. Here is a minimal
//! example to get you started:
//!
//! ```no_run
//! use twitch_irc::login::StaticLoginCredentials;
//! use twitch_irc::ClientConfig;
//! use twitch_irc::SecureTCPTransport;
//! use twitch_irc::TwitchIRCClient;
//!
//! #[tokio::main]
//! pub async fn main() {
//!     // default configuration is to join chat as anonymous.
//!     let config = ClientConfig::default();
//!     let (mut incoming_messages, client) =
//!         TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);
//!
//!     // first thing you should do: start consuming incoming messages,
//!     // otherwise they will back up.
//!     let join_handle = tokio::spawn(async move {
//!         while let Some(message) = incoming_messages.recv().await {
//!             println!("Received message: {:?}", message);
//!         }
//!     });
//!
//!     // join a channel
//!     // This function only returns an error if the passed channel login name is malformed,
//!     // so in this simple case where the channel name is hardcoded we can ignore the potential
//!     // error with `unwrap`.
//!     client.join("sodapoppin".to_owned()).unwrap();
//!
//!     // keep the tokio executor alive.
//!     // If you return instead of waiting the background task will exit.
//!     join_handle.await.unwrap();
//! }
//! ```
//!
//! The above example connects to chat anonymously and listens to messages coming to the channel `sodapoppin`.
//!
//! # Features
//!
//! * Simple API
//! * Integrated connection pool, new connections will be made based on your application's demand
//!   (based on amount of channels joined as well as number of outgoing messages)
//! * Automatic reconnect of failed connections, automatically re-joins channels
//! * Rate limiting of new connections
//! * Support for refreshing login tokens, see below
//! * Fully parses all message types (see [`ServerMessage`](message/enum.ServerMessage.html)
//!   for all supported types)
//! * Can connect using all protocol types supported by Twitch
//! * Supports Rustls as well as Native TLS
//! * No unsafe code
//! * Feature flags to reduce compile time and binary size
//!
//! # Send messages
//!
//! To send messages, use the `TwitchIRCClient` handle you get from `TwitchIRCClient::new`.
//!
//! ```no_run
//! # use twitch_irc::login::StaticLoginCredentials;
//! # use twitch_irc::ClientConfig;
//! # use twitch_irc::SecureTCPTransport;
//! # use twitch_irc::TwitchIRCClient;
//! #
//! # #[tokio::main]
//! # async fn main() {
//! # let config = ClientConfig::default();
//! # let (mut incoming_messages, client) = TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);
//! client.say("a_channel".to_owned(), "Hello world!".to_owned()).await.unwrap();
//! # }
//! ```
//!
//! The `TwitchIRCClient` handle can also be cloned and then used from multiple threads.
//!
//! See the documentation on [`TwitchIRCClient`](struct.TwitchIRCClient.html)
//! for the possible methods.
//!
//! # Receive and handle messages
//!
//! Incoming messages are [`ServerMessage`](message/enum.ServerMessage.html)s. You can use a match
//! block to differentiate between the possible server messages:
//!
//! ```no_run
//! # use twitch_irc::message::ServerMessage;
//! # use tokio::sync::mpsc;
//! #
//! # #[tokio::main]
//! # async fn main() {
//! # let mut incoming_messages: mpsc::UnboundedReceiver<ServerMessage> = unimplemented!();
//! while let Some(message) = incoming_messages.recv().await {
//!      match message {
//!          ServerMessage::Privmsg(msg) => {
//!              println!("(#{}) {}: {}", msg.channel_login, msg.sender.name, msg.message_text);
//!          },
//!          ServerMessage::Whisper(msg) => {
//!              println!("(w) {}: {}", msg.sender.name, msg.message_text);
//!          },
//!          _ => {}
//!      }
//! }
//! # }
//! ```
//!
//! # Logging in
//!
//! `twitch_irc` ships with [`StaticLoginCredentials`](login/struct.StaticLoginCredentials.html)
//! and [`RefreshingLoginCredentials`](login/struct.RefreshingLoginCredentials.html).
//!
//! For simple cases, `StaticLoginCredentials` fulfills all needs:
//!
//! ```
//! use twitch_irc::login::StaticLoginCredentials;
//! use twitch_irc::ClientConfig;
//!
//! let login_name = "your_bot_name".to_owned();
//! let oauth_token = "u0i05p6kbswa1w72wu1h1skio3o20t".to_owned();
//!
//! let config = ClientConfig::new_simple(
//!     StaticLoginCredentials::new(login_name, Some(oauth_token))
//! );
//! ```
//!
//! However for most applications it is strongly recommended to have your login token automatically
//! refreshed when it expires. For this, enable one of the `refreshing-token` feature flags
//! (see [Feature flags](#feature-flags)), and use
//! [`RefreshingLoginCredentials`](login/struct.RefreshingLoginCredentials.html), for example
//! like this:
//!
//! ```no_run
//! # #[cfg(feature = "refreshing-login")] {
//! use async_trait::async_trait;
//! use twitch_irc::login::{RefreshingLoginCredentials, TokenStorage, UserAccessToken};
//! use twitch_irc::ClientConfig;
//!
//! #[derive(Debug)]
//! struct CustomTokenStorage {
//!     // fields...
//! }
//!
//! #[async_trait]
//! impl TokenStorage for CustomTokenStorage {
//!     type LoadError = std::io::Error; // or some other error
//!     type UpdateError = std::io::Error;
//!
//!     async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
//!         // Load the currently stored token from the storage.
//!         Ok(UserAccessToken {
//!             access_token: todo!(),
//!             refresh_token: todo!(),
//!             created_at: todo!(),
//!             expires_at: todo!()
//!         })
//!     }
//!
//!     async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
//!         // Called after the token was updated successfully, to save the new token.
//!         // After `update_token()` completes, the `load_token()` method should then return
//!         // that token for future invocations
//!         todo!()
//!     }
//! }
//!
//! // these credentials can be generated for your app at https://dev.twitch.tv/console/apps
//! // the bot's username will be fetched based on your access token
//! let client_id = "rrbau1x7hl2ssz78nd2l32ns9jrx2w".to_owned();
//! let client_secret = "m6nuam2b2zgn2fw8actt8hwdummz1g".to_owned();
//! let storage = CustomTokenStorage { /* ... */ };
//!
//! let credentials = RefreshingLoginCredentials::new(client_id, client_secret, storage);
//! // It is also possible to use the same credentials in other places
//! // such as API calls by cloning them.
//! let config = ClientConfig::new_simple(credentials);
//! // then create your client and use it
//! # }
//! ```
//!
//! `RefreshingLoginCredentials` needs an implementation of `TokenStorage` that depends
//! on your application, to retrieve the token or update it. For example, you might put the token
//! in a config file you overwrite, some extra file for secrets, or a database.
//!
//! In order to get started with `RefreshingLoginCredentials`, you need to have initial access
//! and refresh tokens present in your storage. You can fetch these tokens using the
//! [OAuth authorization code flow](https://dev.twitch.tv/docs/authentication/getting-tokens-oauth#oauth-authorization-code-flow).
//! There is also a [`GetAccessTokenResponse`](crate::login::GetAccessTokenResponse) helper struct
//! that allows you to decode the `POST /oauth2/token` response as part of the authorization process.
//! See the documentation on that type for details on usage and how to convert the decoded response
//! to a `UserAccessToken` that you can then write to your `TokenStorage`.
//!
//! # Close the client
//!
//! To close the client, drop all clones of the `TwitchIRCClient` handle. The client will shut down
//! and end the stream of incoming messages once all processing is done.
//!
//! # Feature flags
//!
//! This library has these optional feature toggles:
//! * **`transport-tcp`** enables `PlainTCPTransport`, to connect using a plain-text TLS socket
//!   using the normal IRC protocol.
//!     * `transport-tcp-native-tls` enables `SecureTCPTransport` which will then use OS-native
//!        TLS functionality to make a secure connection. Root certificates are the ones configured
//!        in the operating system.
//!     * `transport-tcp-rustls-native-roots` enables `SecureTCPTransport` using [Rustls][rustls]
//!        as the TLS implementation, but will use the root certificates configured in the
//!        operating system.
//!     * `transport-tcp-rustls-webpki-roots` enables `SecureTCPTransport` using [Rustls][rustls]
//!        as the TLS implementation, and will statically embed the current
//!        [Mozilla root certificates][mozilla-roots] as the trusted root certificates.
//! * **`transport-ws`** enables `PlainWSTransport` to connect using the Twitch-specific websocket
//!   method. (Plain-text)
//!     * `transport-ws-native-tls` further enables `SecureWSTransport` which will then use OS-native
//!        TLS functionality to make a secure connection. Root certificates are the ones configured
//!        in the operating system.
//!     * `transport-ws-rustls-native-roots` enables `SecureWSTransport` using [Rustls][rustls]
//!        as the TLS implementation, but will use the root certificates configured in the
//!        operating system.
//!     * `transport-ws-rustls-webpki-roots` enables `SecureWSTransport` using [Rustls][rustls]
//!        as the TLS implementation, and will statically embed the current
//!        [Mozilla root certificates][mozilla-roots] as the trusted root certificates.
//! * Three different feature flags are provided to enable the
//!   [`RefreshingLoginCredentials`](crate::login::RefreshingLoginCredentials):
//!     * `refreshing-token-native-tls` enables this feature using the OS-native TLS functionality
//!        to make secure connections. Root certificates are the ones configured
//!        in the operating system.
//!     * `refreshing-token-rustls-native-roots` enables this feature using
//!        [Rustls][rustls] as the TLS implementation, but will use the root certificates configured
//!        in the operating system.
//!     * `refreshing-token-rustls-webpki-roots` enables this feature using [Rustls][rustls]
//!        as the TLS implementation, and will statically embed the current [Mozilla root
//!        certificates][mozilla-roots] as the trusted root certificates.
//! * **`metrics-collection`** enables a set of metrics to be exported from the client. See the
//!   documentation on `ClientConfig` for details.
//! * **`with-serde`** pulls in `serde` v1.0 and adds `#[derive(Serialize, Deserialize)]` to many
//!   structs. This feature flag is automatically enabled when using any of the `refreshing-token`
//!   feature flags.
//!
//! By default, `transport-tcp` and `transport-tcp-native-tls` are enabled.
//!
//! [rustls]: https://github.com/ctz/rustls
//! [mozilla-roots]: https://github.com/ctz/webpki-roots

pub mod client;
mod config;
mod connection;
mod error;
pub mod login;
pub mod message;
#[cfg(feature = "metrics-collection")]
mod metrics;
pub mod transport;
pub mod validate;

pub use client::TwitchIRCClient;
pub use config::ClientConfig;
#[cfg(feature = "metrics-collection")]
pub use config::MetricsConfig;
pub use error::Error;

#[cfg(feature = "transport-tcp")]
pub use transport::tcp::PlainTCPTransport;
#[cfg(all(
    feature = "transport-tcp",
    any(
        feature = "transport-tcp-native-tls",
        feature = "transport-tcp-rustls-native-roots",
        feature = "transport-tcp-rustls-webpki-roots"
    )
))]
pub use transport::tcp::SecureTCPTransport;

#[cfg(feature = "transport-ws")]
pub use transport::websocket::PlainWSTransport;
#[cfg(all(
    feature = "transport-ws",
    any(
        feature = "transport-ws-native-tls",
        feature = "transport-ws-rustls-native-roots",
        feature = "transport-ws-rustls-webpki-roots",
    )
))]
pub use transport::websocket::SecureWSTransport;

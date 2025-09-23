use rustyline::Editor;
use rustyline::history::DefaultHistory;

mod completer;
use completer::CommandCompleter;

use anyhow::Result;
use chrono::Local;
use clap::Parser;
use once_cell::sync::Lazy;
use owo_colors::OwoColorize;
use rustyline::error::ReadlineError;

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self,Write},
    sync::{Arc, Mutex},
    process,
    process::Command,
};
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::{PrivmsgMessage, ServerMessage};
use twitch_irc::message::ClearChatAction;
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};
use chrono::prelude::*;
use chrono_tz::Europe::Berlin;
mod channel_config; // declares the module
use channel_config::{ChannelConfig, load_channel_config, apply_named_color};

mod sound;
use sound::{play_sound, WaveformType, SOUND_CONTROLLER};
use home;



static CONFIG: Lazy<ChannelConfig> = Lazy::new(|| {
    let config_path = match home::home_dir() {
        Some(mut path) => {
            path.push(".rustTwitchLogger/channels.txt");
            path
        }
        None => {
            eprintln!("⚠️ Error: Unable to find the home directory to load channels.txt.");
            process::exit(1);
        }
    };

    // Convert the PathBuf to a &str, handling the possibility of an error.
    if let Some(path_str) = config_path.to_str() {
        match load_channel_config(path_str) { // <-- Now passing a valid &str
            Ok(cfg) => cfg,
                                               Err(e) => {
                                                   eprintln!(
                                                       "⚠️ Error: Failed to load configuration from '{}': {}",
                                                       config_path.display(),
                                                             e
                                                   );
                                                   process::exit(1);
                                               }
        }
    } else {
        eprintln!(
            "⚠️ Error: The configuration path '{}' is not valid UTF-8.",
            config_path.display()
        );
        process::exit(1);
    }
});

static STARTUP_DATE: Lazy<String> = Lazy::new(|| {
    let now = Utc::now().with_timezone(&Berlin);
    // Get the abbreviated weekday (e.g., "Sa")
    let day_abbr = &now.format("%a").to_string()[0..2];
    format!("{}_{}", day_abbr, now.format("%d_%m_%Y"))
});


// --- Command-Line Argument Parser ---
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// List of Twitch channels to join
    #[arg(name = "CHANNELS")]
    channels: Vec<String>,
}


fn get_local_timestamp() -> String {
    let now = Utc::now().with_timezone(&Berlin);
    // Get the abbreviated weekday (e.g., "Sat") and take the first two letters.
    let day_abbr = &now.format("%a").to_string()[0..2];

    format!(
        "{}_{}", // Prepend the two-letter day to the rest of the timestamp
        day_abbr,
        now.format("%d_%m_%Y_%H-%M-%S")
    )
}

use notify_rust::{Notification, Timeout};

const DEFAULT_TIMEOUT: Option<u32> = Some(12000);

fn send_desktop_notification(summary: &str, body: &str, timeout_ms: Option<u32>) {
    let mut notification = Notification::new();
    notification.summary(summary).body(body);

    if let Some(ms) = timeout_ms {
        // Set a custom timeout in milliseconds
        notification.timeout(Timeout::Milliseconds(ms));
    }
    // If timeout_ms is None, the notification server's default timeout will be used.

    if let Err(e) = notification.show() {
        eprintln!("⚠️ Failed to send notification: {}", e);
    }
}
// --- Main Application Logic ---
#[tokio::main]
async fn main() -> Result<()> {

    println!("last update: 14.08.25");

    use tokio::sync::oneshot;
    let cli = Cli::parse();
    //let (exit_tx, exit_rx) = oneshot::channel();
    let (exit_tx, exit_rx) = oneshot::channel::<()>();


    let initial_channels: Vec<String> = if cli.channels.is_empty() {
        CONFIG.default_channels.iter().cloned().collect()
    } else {
        cli.channels
    };

    let client_config = ClientConfig::default();
    let (mut incoming_messages, client) =
    TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(client_config);

    // --- Shared State ---
    let channels        = Arc::new(Mutex::new(initial_channels.clone()));
    let logs            = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    let join_logs       = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    let sound_channels  = Arc::new(Mutex::new(
        initial_channels.iter().cloned().collect::<HashSet<String>>(),
    ));

    let notification_channels = Arc::new(Mutex::new(HashSet::<String>::new()));





    // --- Join Initial Channels ---
    for channel in &initial_channels {
        client.join(channel.clone())?;
        println!("Joined initial channel: {}", channel.green());
    }

    // --- Message Handling Task ---
    let logs_for_tokio          = Arc::clone(&logs);
    let join_logs_for_tokio = Arc::clone(&join_logs);


    let sound_channels_for_tokio = Arc::clone(&sound_channels);
    let notification_channels_for_tokio = Arc::clone(&notification_channels);

    let join_handle = tokio::spawn(async move {
        tokio::select! {
            _ = async {
                while let Some(message) = incoming_messages.recv().await {
                    let time_str = Local::now().format("%H:%M:%S").to_string();
                    match message {
                        ServerMessage::Privmsg(msg) => {
                            handle_privmsg(&time_str, msg, &logs_for_tokio, &sound_channels_for_tokio,&notification_channels_for_tokio);
                        }

                        ServerMessage::Join(msg) =>{
                           handle_join_or_part("JOIN", &time_str, &msg.channel_login, &msg.user_login, &logs_for_tokio, &join_logs_for_tokio);
                        }

                        ServerMessage::Part(msg) => {
                            handle_join_or_part("PART", &time_str, &msg.channel_login, &msg.user_login, &logs_for_tokio, &join_logs_for_tokio);
                        }

                        ServerMessage::Ping(_msg) => {
                            print!("{} PING      \r", time_str); // Padding to overwrite leftover text
                            io::stdout().flush().unwrap();
                        }
                        ServerMessage::Pong(_msg) => {
                            print!("{} PONG      \r", time_str); // Same here
                            io::stdout().flush().unwrap();
                        }
                        ServerMessage::RoomState(_msg) =>{}

                        ServerMessage::Notice(msg) => {
                            println!("{}[{}][NOTICE] {}", time_str.dimmed(), msg.channel_login.unwrap_or("unknown".to_string()),msg.message_text);
                        }

                        ServerMessage::ClearChat(msg) => {
                            match &msg.action {
                                ClearChatAction::UserBanned { user_login, .. } => {
                                    handle_moderation_event(
                                        &time_str,
                                        "USER_BANNED",
                                        &msg.channel_login,
                                        user_login,
                                        owo_colors::Style::new().red().blink(),
                                                            &logs_for_tokio, // Or your new moderation_logs store
                                    );
                                }
                                ClearChatAction::UserTimedOut { user_login, timeout_length, .. } => {
                                    let content = format!(
                                        "{} ({}s timeout)",
                                                          user_login,
                                                          timeout_length.as_secs()
                                    );
                                    handle_moderation_event(
                                        &time_str,
                                        "TIMEOUT",
                                        &msg.channel_login,
                                        &content,
                                        owo_colors::Style::new().red().blink(),
                                                            &logs_for_tokio, // Or your new moderation_logs store
                                    );
                                }
                                ClearChatAction::ChatCleared => {
                                    handle_moderation_event(
                                        &time_str,
                                        "CHAT_CLEARED",
                                        &msg.channel_login,
                                        "The chat was cleared by a moderator.",
                                        owo_colors::Style::new().dimmed(),
                                                            &logs_for_tokio, // Or your new moderation_logs store
                                    );
                                }
                            }
                        }
                        ServerMessage::ClearMsg(msg) => {
                            handle_moderation_event(
                                &time_str,
                                "CLEARMSG",
                                &msg.channel_login,
                                &msg.message_text,
                                owo_colors::Style::new().bright_black().blink(),
                                                    &logs_for_tokio,
                            );
                        }
                        ServerMessage::UserNotice(msg) => {
                            handle_user_notice(&time_str, &msg, &logs_for_tokio);
                        }

                        _ => handle_default(&time_str, &message, &logs_for_tokio),
                    }
                }
            } => {},
            _ = exit_rx => {
                println!("Message loop received exit signal.");
            }
        }
    });


    // --- User Input Handling Thread ---


    let client_for_thread = client.clone();
    let logs_for_thread = Arc::clone(&logs);
     let join_logs_for_thread = Arc::clone(&join_logs);

    let vips: Vec<String> = CONFIG.vips.keys().cloned().collect();

    let msg_logs: Vec<String> = {
        let logs_guard = logs.lock().unwrap();
        logs_guard.keys().cloned().collect()
    };

    let channels_for_thread = Arc::clone(&channels);
    let sound_channels_for_thread = Arc::clone(&sound_channels);
    let notification_channels_for_thread = Arc::clone(&notification_channels);

    let handle = std::thread::spawn(move || -> Result<()> {
        let commands = vec![
            "JOIN".into(),
                                    "PART".into(),
                                    "SOUND".into(),
                                    "SAVE".into(),
                                    "NOTIFY".into(),
                                    "EXIT".into(),
                                    "RECONNECT".into(),
                                    "PAUSES".into(),
                                    "STATS".into(),
                                    "FREQ".into(),
                                    "WAVE".into()
        ];

        let waveforms = vec![
            "SQUARE".into(),
                                    "SINE".into(),
                                    "SAWTOOTH".into(),
                                    "TRIANGLE".into(),
        ];

        let completer = CommandCompleter {
            commands: commands.clone(),
                                    waveforms: waveforms.clone(), // <-- Add this line
                                    joined_channels: Arc::clone(&channels_for_thread),
                                    vips: vips.clone(),
                                    log_channels: Arc::clone(&logs_for_thread),
        };

        let mut rl = Editor::<CommandCompleter, DefaultHistory>::new()?;
        rl.set_helper(Some(completer));

        println!("Commands: JOIN, PART, SOUND, FREQ <hz>, WAVE <type>, SAVE, EXIT");

        loop {
            match rl.readline(">> ") {
                Ok(input) => {
                    let _ = rl.add_history_entry(input.as_str());
                    let parts: Vec<&str> = input.trim().split_whitespace().collect();
                    if parts.is_empty() {
                        continue;
                    }

                    let cmd = parts[0].to_uppercase();
                    let arg = parts.get(1).map(|s| s.to_string());

                    match cmd.as_str() {
                        "JOIN" => {
                            if let Some(channel) = arg {
                                let _ = client_for_thread.join(channel.clone());
                                channels_for_thread.lock().unwrap().push(channel.clone());
                                println!("Joined {}", channel.green());
                            }
                        },
                        "PART" => {
                            if let Some(channel) = arg {
                                let _ = client_for_thread.part(channel.clone());
                                channels_for_thread.lock().unwrap().retain(|c| c != &channel);
                                println!("Parted from {}", channel.red());
                            }
                        },
                        "SOUND" => {
                            if let Some(channel) = arg {
                                let mut sound_chans = sound_channels_for_thread.lock().unwrap();
                                if sound_chans.contains(&channel) {
                                    sound_chans.remove(&channel);
                                    println!("Sound OFF for {}", channel.yellow());
                                } else {
                                    sound_chans.insert(channel.clone());
                                    notification_channels_for_thread.lock().unwrap().remove(&channel);
                                    println!("Sound ON for {}", channel.green());
                                }
                            }
                        },
                        "NOTIFY" => {
                            if let Some(channel) = arg {
                                let mut notify_chans = notification_channels_for_thread.lock().unwrap();
                                if notify_chans.contains(&channel) {
                                    // It was on, so turn it off
                                    notify_chans.remove(&channel);
                                    println!("Notifications OFF for {}", channel.yellow());
                                } else {
                                    // It was off, so turn it on and ensure sound is off
                                    notify_chans.insert(channel.clone());
                                    sound_channels_for_thread.lock().unwrap().remove(&channel);
                                    println!("Notifications ON for {} (Sound is now OFF)", channel.cyan());
                                }
                            }
                        },
                        "SAVE" => {
                            if parts.len() >= 2 {
                                let target = parts[1];
                                let custom_name = if parts.len() > 2 {
                                    Some(parts[2..].join("_"))
                                } else {
                                    None
                                };
                                save_logs(
                                    target,
                                    &logs_for_thread,
                                    &join_logs_for_thread,
                                    custom_name.as_deref()
                                );
                            } else {
                                println!("Usage: SAVE <channel|ALL> [optional_custom_name]");
                            }
                        },
                        "FREQ" => {
                            if let Some(freq_str) = arg {
                                match freq_str.parse::<f32>() {
                                    Ok(new_freq) => {
                                        // Lock the mutex and update the value
                                        let mut freq = SOUND_CONTROLLER.frequency.lock().unwrap();
                                        *freq = new_freq;
                                        println!("Sound frequency set to {} Hz", new_freq.cyan());
                                    }
                                    Err(_) => {
                                        println!("'{}' is not a valid frequency.", freq_str.red());
                                    }
                                }
                            } else {
                                // Print the current frequency if no argument is given
                                let current_freq = *SOUND_CONTROLLER.frequency.lock().unwrap();
                                println!("Current sound frequency is {} Hz", current_freq.cyan());
                            }
                        },
                        "WAVE" => {
                            // Check if an argument was provided
                            if let Some(wave_arg) = arg {
                                // Convert the argument to uppercase before matching
                                let new_wave = match wave_arg.to_uppercase().as_str() {
                                    "SQUARE" => Some(WaveformType::Square),
                                    "SINE" => Some(WaveformType::Sine),
                                    "SAWTOOTH" => Some(WaveformType::Sawtooth),
                                    "TRIANGLE" => Some(WaveformType::Triangle),
                                    _ => None, // The input didn't match any known type
                                };

                                if let Some(wave) = new_wave {
                                    *SOUND_CONTROLLER.waveform.lock().unwrap() = wave;
                                    println!("Sound waveform set to {:?}", wave.cyan());
                                } else {
                                    // Give a more specific error message if the input was invalid
                                    println!("Unknown waveform: '{}'. Use SQUARE, SINE, SAWTOOTH, or TRIANGLE.", wave_arg.red());
                                }
                            } else {
                                // No argument was given, print the current state
                                let current_wave = *SOUND_CONTROLLER.waveform.lock().unwrap();
                                println!(
                                    "Usage: WAVE <type>. Current waveform is: {:?}",
                                    current_wave.cyan()
                                );
                            }
                        },
                        "EXIT" => {
                            println!("Shutting down...");
                            let joined_channels = channels_for_thread.lock().unwrap().clone();
                            for channel in joined_channels {
                                let _ = client_for_thread.part(channel.clone());
                                println!("Left channel: {}", channel);
                            }
                            let _ = exit_tx.send(()); // notify the async task
                            break;
                        },
                        _ => println!("{}: '{}'", "Unknown command".red(), input.trim()),
                    }
                }
                Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                    println!("Exiting...");
                    break;
                }
                Err(err) => {
                    println!("Input Error: {:?}", err);
                    break;
                }
            }
        }

        Ok(())
    });
    let handle_result = handle.join().expect("Input thread panicked");
    if let Err(e) = handle_result {
        eprintln!("Error in input thread: {:?}", e);
    }

    join_handle.await?;

    Ok(())
}
// --- Message Handlers ---

fn handle_default(
    time: &str,
    message: &ServerMessage,
    _logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
) {
    use twitch_irc::message::ServerMessage;

    let kind = match message {
        ServerMessage::Ping(_) => "PING",
        ServerMessage::Pong(_) => "PONG",
        ServerMessage::Reconnect(_) => "RECONNECT",
        ServerMessage::GlobalUserState(_) => "GLOBAL_USER_STATE",
        ServerMessage::UserState(_) => "USER_STATE",
        ServerMessage::RoomState(_) => "ROOM_STATE",
        ServerMessage::Whisper(_) => "WHISPER",
        ServerMessage::Generic(_)=> "HIDDEN",
        _ => "OTHER",
    };

    if kind == "OTHER" {
        println!("{} [SYSTEM: OTHER] {:?}", time.dimmed(), message
        .source()
        .tags
        .0
        .get("msg-id")
        .and_then(|v| v.as_deref())
        .unwrap_or("unknown"));
    } else {
        println!("{} ...", time.dimmed())
    }
}

fn handle_privmsg(
    time_str: &str,
    msg: PrivmsgMessage,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    sound_channels: &Arc<Mutex<HashSet<String>>>,
    notification_channels: &Arc<Mutex<HashSet<String>>>
) {

    // Use vips for colorized printing
    let info = CONFIG.vips.get(&msg.channel_login);
    let channel_display = apply_named_color(&msg.channel_login, info.and_then(|c| c.color.as_deref()));

    let mut custom_badges = msg.badges.iter()
    .map(|b| format!("{}/{}", b.name, b.version))
    .collect::<Vec<_>>();

    let tags = &msg.source.tags;

    // Add virtual badges based on tag fields
    if let Some(first_msg) = tags.0.get("first-msg").and_then(|v| v.as_deref()) {
        if first_msg == "1" {
            custom_badges.push("(FIRSTMSG)".to_string());
        }
    }

    if let Some(returning) = tags.0.get("returning-chatter").and_then(|v| v.as_deref()) {
        if returning == "1" {
            custom_badges.push("(RETURNING)".to_string());
        }
    }

    let badges_for_log = custom_badges.join(",");
    let badge_info_for_console = if !custom_badges.is_empty() {
        format!("[{}]", custom_badges.join(", ").yellow())
    } else {
        String::new()
    };

    let log_line = format!(
        "{} <{}>{}\n{}\n",
        time_str,
        msg.sender.name,
        if badges_for_log.is_empty() {
            "".to_string()
        } else {
            format!(" [{}]", badges_for_log.replace("moderator/","mod/").replace("subscriber/","sub/").replace("premium/","prime/"))
        },//badges at the end in the logfile
        msg.message_text
    );

    logs.lock().unwrap().entry(msg.channel_login.clone()).or_default().push(log_line);

    // --- END OF BADGE LOGIC ---

    let user_styled = if let Some(color) = msg.name_color {
        msg.sender.name.truecolor(color.r, color.g, color.b).to_string()
    } else {
        msg.sender.name.clone()
    };

    println!(
        "{} [{}] {}{}: {}",
        time_str.dimmed(),
             channel_display,
             user_styled.bold(),
             badge_info_for_console.replace("moderator/","mod/").replace("subscriber/","sub/").replace("premium/","prime/"),
             msg.message_text
    );

    let summary = format!("#{}", msg.channel_login);
    let body = format!("{}: {}", msg.sender.name, msg.message_text);

    let calculate_timeout = || -> Option<u32> {
        const BASE_TIMEOUT_MS: u32 = 3000; // 3 seconds base time
        const MAX_TIMEOUT_MS: u32 = 20000; // 20 seconds max time
        const WORDS_PER_MINUTE: f64 = 180.0; // Slower reading speed for comfort

        let word_count = msg.message_text.split_whitespace().count();
        if word_count == 0 {
            return Some(BASE_TIMEOUT_MS);
        }

        // Calculate reading time in milliseconds
        // (word_count / WPM) -> minutes | * 60 -> seconds | * 1000 -> milliseconds
        let reading_time_ms = ((word_count as f64 / WORDS_PER_MINUTE) * 60.0 * 1000.0) as u32;

        let total_timeout = (BASE_TIMEOUT_MS + reading_time_ms).min(MAX_TIMEOUT_MS);
        Some(total_timeout)
    };


    if sound_channels.lock().unwrap().contains(&msg.channel_login) {

        send_desktop_notification(&summary, &body, calculate_timeout());
        play_sound();
    }else if notification_channels.lock().unwrap().contains(&msg.channel_login) {
        // Notify mode: only sends a notification
        send_desktop_notification(&summary, &body, calculate_timeout());
    }
}

/*https://docs.rs/twitch-irc/latest/twitch_irc/message/enum.UserNoticeEvent.html*/

fn handle_user_notice(
    time: &str,
    msg: &twitch_irc::message::UserNoticeMessage,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
) {
    use owo_colors::OwoColorize;
    use twitch_irc::message::UserNoticeEvent;

    // Fallback to raw msg-id tag if the event is unknown
    let raw_msg_id = msg
    .source
    .tags
    .0
    .get("msg-id")
    .and_then(|v| v.as_deref())
    .unwrap_or("unknown");

    let event_type = match &msg.event {
        UserNoticeEvent::Unknown => raw_msg_id.to_uppercase(),
        other => format!("{:?}", other).to_uppercase(),
    };

    let channel = &msg.channel_login;
    let user = &msg.sender.name;
    let user_msg = msg.message_text.as_deref().unwrap_or("");
    let sys_msg = msg.system_message.trim();

    // Compose log line
    let line = format!(
        "{} [{}][{}] <{}> {} → {}",
        time,
        channel,
        user,
        event_type,
        user_msg,
        sys_msg
    );

    println!(
        "{} [{}][{}] {}: {}\n→ {}",
        time.dimmed(),
             channel,
             user,
             event_type.blue(),
             user_msg,
             sys_msg.yellow()
    );

    if let Ok(mut logs) = logs.lock() {
        logs.entry(channel.clone())
        .or_default()
        .push(line);
    }
}


fn handle_moderation_event(
    time_str: &str,
    event_type: &str,
    channel: &str,
    content: &str,
    style: owo_colors::Style,
    log_store: &Arc<Mutex<HashMap<String, Vec<String>>>>,
) {
    let log_line = format!("{time_str} {event_type}: [#{channel}] {content}");
    println!("{}", log_line.style(style));

    let summary = format!("Moderation in #{}", channel);
    let body = format!("[{}] {}", event_type, content);
    send_desktop_notification(&summary, &body, Some(40000));
    play_sound();


    let mut logs = log_store.lock().unwrap();
    logs.entry(channel.to_string()).or_default().push(log_line);
}



fn handle_join_or_part(
     event_type: &str,
     time_str: &str,
     channel: &str,
     username: &str,
     log_store: &Arc<Mutex<HashMap<String, Vec<String>>>>,
     join_log_store: &Arc<Mutex<HashMap<String, Vec<String>>>>,
  ){

     let msg = format!("{time_str} [{event_type}] {username}");
     join_log_store.lock().unwrap()
     .entry(channel.to_string())
     .or_default()
     .push(msg.clone().replace("[JOIN] ","[J] ").replace("[PART] ","[P] "));

     if CONFIG.vips.contains_key(username) {
         println!("{}", format!("*** VIP {username} has {event_type}ed {channel} ***").yellow());


         // Save in general log when it's a VIP, but on same channel
        if username != channel {
         log_store.lock().unwrap()
         .entry(channel.to_string())
         .or_default()
         .push(msg.clone());
        }

         if event_type == "JOIN" && username != channel {
             play_sound();
             send_desktop_notification(channel, &format!("{} joined",username), Some(4000));
         }
     }
}

// --- Utility Functions ---
fn save_logs(
    target: &str,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    join_logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    // The `first_message_times` parameter is now gone
    custom_name: Option<&str>,
) {
    let logs_locked = logs.lock().unwrap();
    let join_logs_locked = join_logs.lock().unwrap();

    let targets: Vec<String> = if target.eq_ignore_ascii_case("ALL") {
        logs_locked.keys().cloned().collect()
    } else {
        vec![target.to_string()]
    };

    for chan in targets {
        // --- NEW LOGIC: Get time from the first log entry ---
        let time_part = logs_locked
        .get(&chan)
        // Find the first message in the log vector for this channel
        .and_then(|messages| messages.iter().find(|line| line.contains("<") && line.contains(">")))
        // Parse the timestamp (HH:MM:SS) from the beginning of the line
        .map(|first_line| first_line[0..8].replace(':', "-")) // "HH:MM:SS" -> "HH-MM-SS"
        // If no messages exist for the channel, use the current time as a fallback
        .unwrap_or_else(|| Local::now().format("%H-%M-%S").to_string());

        // Combine the static date part with the parsed time part.
        let timestamp = format!("{}_{}", *STARTUP_DATE, time_part);

        // --- Save the main message log ---
        if let Some(messages) = logs_locked.get(&chan) {
            let file = if let Some(name) = custom_name {
                format!("/tmp/{}_{}_{}.txt", chan, name, timestamp)
            } else {
                format!("/tmp/{}_msgs_{}.txt", chan, timestamp)
            };

            let mut msg_count = 0;
            let mut unique_chatters = HashSet::new();
            let mut mod_events = 0;
            let mut sub_events = 0;
            let mut raid_events = 0;

            for line in messages {
                if line.contains("<SUBORRESUB") || line.contains("<SUBGIFT") || line.contains("<SUBMYSTERYGIFT")
                    || line.contains("<ANONSUBMYSTERYGIFT") || line.contains("<GIFTPAIDUPGRADE") || line.contains("ANONPAIDGIFTUPGRADE") {
                        sub_events += 1;
                    } else if line.contains("USER_BANNED") || line.contains("CLEARMSG") || line.contains("TIMEOUT") {
                        mod_events += 1;
                    } else if line.contains("<RAID") {
                        raid_events += 1;
                    } else if line.matches("<").count() == 1 && line.contains(">") {
                        msg_count += 1;
                        if let Some(start) = line.find('<') {
                            if let Some(end) = line.find('>') {
                                let username = &line[start + 1..end];
                                if let Some(uname) = username.split(']').last() {
                                    unique_chatters.insert(uname.trim().to_string());
                                }
                            }
                        }
                    }
            }

            let header = format!(
                "--- Message/Event Log ---\n# {}\n({} messages from {} chatters)\n({} Banns, Deletions, and Timeouts)\n({} Subs/Giftsubs)\n({} Raids)\n",
                                 chan,
                                 msg_count,
                                 unique_chatters.len(),
                                 mod_events,
                                 sub_events,
                                 raid_events
            );

            let numbered_messages = messages
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{}. {}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

            let final_content = format!("{}{}", header, numbered_messages);

            let mut content_with_bom = vec![0xEF, 0xBB, 0xBF];
            content_with_bom.extend_from_slice(final_content.as_bytes());

            if let Ok(mut f) = File::create(&file) {
                if f.write_all(&content_with_bom).is_ok() {
                    println!("Saved {} messages to {}", messages.len(), file);
                }
            }
        }


        // --- Save the join/part log to a separate file ---
        if let Some(join_msgs) = join_logs_locked.get(&chan) {
            if !join_msgs.is_empty() {
                let file = if let Some(name) = custom_name {
                    format!("/tmp/{}_{}_joins_{}.txt", chan, name, timestamp)
                } else {
                    format!("/tmp/{}_joins_{}.txt", chan, timestamp)
                };

                if std::fs::write(&file, join_msgs.join("\n")).is_ok() {
                    println!("Saved {} JOIN/PART events to {}", join_msgs.len(), file);
                }
            }
        }
    }
}


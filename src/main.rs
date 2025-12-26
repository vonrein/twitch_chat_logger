use rustyline::Editor;
use rustyline::history::DefaultHistory;



mod completer;
use completer::CommandCompleter;

mod globals;
use globals::IS_SERVER_MODE;

use anyhow::Result;
use chrono::Local;
use clap::Parser;
use once_cell::sync::Lazy;
use owo_colors::OwoColorize;
use rustyline::error::ReadlineError;

use std::{
    collections::{HashMap, HashSet},
    fs::{File, create_dir_all, read_to_string},
    io::{self,Write},
    sync::{Arc, Mutex},
    path::PathBuf,
    process,
    time::{Duration, Instant},
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
use sound::{play_sound};
use home;
use rand::Rng;


//#[cfg(not(target_os = "windows"))] // Only use on Linux/Unix
//use tikv_jemallocator::Jemalloc;

//#[cfg(not(target_os = "windows"))]
//#[global_allocator]
//static GLOBAL: Jemalloc = Jemalloc;

static RANDOM_COLORS: Lazy<Mutex<HashMap<String, (u8, u8, u8)>>> = Lazy::new(|| {
    Mutex::new(HashMap::new())
});

fn get_or_assign_channel_color(channel: &str) -> (u8, u8, u8) {
    let mut colors = RANDOM_COLORS.lock().unwrap();
    *colors.entry(channel.to_string()).or_insert_with(|| {
        let mut rng = rand::rng();
        let (r, g, b): (u8, u8, u8) = (rng.random(), rng.random(), rng.random());
        println!("Assigned random color for {}: #{:02X}{:02X}{:02X}",channel.truecolor(r, g, b), r, g, b);
        (r, g, b)
    })
}


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


static SESSION_TIMESTAMP: Lazy<String> = Lazy::new(|| {
    let now = Utc::now().with_timezone(&Berlin);
    // Format: Day_Date_Time (e.g., Mon_26_10_2025_17-06-33)
    let day_abbr = &now.format("%a").to_string()[0..2];
    format!("{}_{}_{}", day_abbr, now.format("%d_%m_%Y"), now.format("%H-%M-%S"))
});


static LOG_DIRECTORY: Lazy<String> = Lazy::new(|| {
    let config_path = match home::home_dir() {
        Some(mut path) => {
            path.push(".rustTwitchLogger/log_path.txt");
            path
        }
        None => {
            eprintln!("⚠️ Error: Unable to find the home directory. Defaulting log path to /tmp.");
            return "/tmp".to_string();
        }
    };

    match read_to_string(&config_path) {
        Ok(path_str) => {
            let trimmed_path = path_str.trim().to_string();
            if trimmed_path.is_empty() {
                eprintln!("⚠️ Warning: log_path.txt is empty. Defaulting log path to /tmp.");
                "/tmp".to_string()
            } else {
                println!("✅ Log directory set to: {}", trimmed_path.cyan());
                trimmed_path
            }
        }
        Err(_) => {
            eprintln!(
                "⚠️ Warning: No log_path.txt found at {}. Defaulting log path to /tmp.",
                config_path.display()
            );
            "/tmp".to_string()
        }
    }
});


// --- Command-Line Argument Parser ---
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// List of Twitch channels to join
    #[arg(name = "CHANNELS")]
    channels: Vec<String>,

    /// Run in server mode (disables sound, notifications, and console logging)
    #[arg(long)]
    server: bool,

    #[arg(long)]
    quiet: bool
}

use notify_rust::{Notification, Timeout, Hint, Urgency};

const DEFAULT_TIMEOUT: Option<u32> = Some(12000);

fn send_desktop_notification(summary: &str, body: &str, timeout_ms: Option<u32>, urgency: Urgency) {

    if *IS_SERVER_MODE.lock().unwrap() {
        return;
    }

    // Use the provided timeout, or fall back to the global default.
    let calculated_timeout = timeout_ms.or(DEFAULT_TIMEOUT).unwrap_or(5000); // 5s absolute fallback

    let final_timeout;
    {
        let mut state = NOTIFICATION_STATE.lock().unwrap();

        // Check if the last notification is still considered "active"
        if state.last_shown.elapsed() < Duration::from_millis(state.last_duration_ms as u64) {
            // If so, the new notification must last for at least as long as the previous one.
            final_timeout = calculated_timeout.max(state.last_duration_ms);
        } else {
            // Otherwise, just use its own calculated timeout.
            final_timeout = calculated_timeout;
        }

        // Update the state for the *next* notification that will come in.
        state.last_duration_ms = final_timeout;
        state.last_shown = Instant::now();
    }

    // Now, build and show the notification with the adjusted timeout.
    let mut notification = Notification::new();
    notification
    .summary(summary)
    .body(body)
    .timeout(Timeout::Milliseconds(final_timeout))
    .hint(Hint::Urgency(urgency)); // <--- Apply the urgency here

    if let Err(e) = notification.show() {
        eprintln!("⚠️ Failed to send notification: {}", e);
    }
}

static NOTIFICATION_STATE: Lazy<Mutex<NotificationState>> = Lazy::new(|| {
    Mutex::new(NotificationState {
        // Initialize to the distant past so the first notification isn't affected
        last_shown: Instant::now() - Duration::from_secs(999),
               last_duration_ms: 0,
    })
});

struct NotificationState {
    last_shown: Instant,
    last_duration_ms: u32,
}

// --- Main Application Logic ---
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    println!("last update: 26.10.25");



    use tokio::sync::oneshot;
    let cli = Cli::parse();
    //let (exit_tx, exit_rx) = oneshot::channel();
    let (exit_tx, exit_rx) = oneshot::channel::<()>();

    if cli.server {
        *IS_SERVER_MODE.lock().unwrap() = true;
    }

    if cli.quiet {
        sound::IS_SOUND_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
        println!("Sound system globally disabled via --quiet.");
    }


    let initial_channels: Vec<String> = if cli.channels.is_empty() {
        CONFIG.default_channels.iter().cloned().collect()
    } else {
        cli.channels
    };

    let client_config = ClientConfig::default();
    let (mut incoming_messages, client) =
    TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(client_config);

    // --- Shared State ---
    let channels          = Arc::new(Mutex::new(initial_channels.clone()));
    let logs              = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    let join_logs         = Arc::new(Mutex::new(Vec::<String>::new()));
    let channel_titles    = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let reconnect_times   = Arc::new(Mutex::new(Vec::<String>::new())); // <-- NEW: Store reconnect times
    let user_ids = Arc::new(Mutex::new(HashMap::<String, String>::new()));


    let (init_sound, init_notify): (HashMap<String, Urgency>, HashMap<String, Urgency>) = if cli.server {
        // Server mode: Nothing on
        (HashMap::new(), HashMap::new())
    } else if cli.quiet {
        // Quiet mode: Populate notifications only
        let mut map = HashMap::new();
        for chan in &initial_channels {
            // Logic: VIPs = Normal, Others = Low
            let urgency = if CONFIG.vips.contains_key(chan) { Urgency::Normal } else { Urgency::Low };
            map.insert(chan.clone(), urgency);
        }
        (HashMap::new(), map)
    } else {
        // Standard mode: Populate Sound (priority)
        let mut map = HashMap::new();
        for chan in &initial_channels {
            // Logic: VIPs = Normal, Others = Low
            let urgency = if CONFIG.vips.contains_key(chan) { Urgency::Normal } else { Urgency::Low };
            map.insert(chan.clone(), urgency);
        }
        (map, HashMap::new())
    };

    let sound_channels = Arc::new(Mutex::new(init_sound));
    let notification_channels = Arc::new(Mutex::new(init_notify));

    // --- REPLACED BLOCK for msg_logging_channels ---
    let initial_msg_logging: HashSet<String> = if cli.server {
        println!("Running in --server mode: Console message logging disabled by default.");
        HashSet::new() // Server mode: start with an empty set
    } else {
        initial_channels.iter().cloned().collect::<HashSet<String>>() // Normal mode: log all
    };
    let msg_logging_channels = Arc::new(Mutex::new(initial_msg_logging));





    // --- Join Initial Channels ---
    for channel in &initial_channels {
        client.join(channel.clone())?;
        println!("Joined initial channel: {}", channel.green());
    }

    // --- Message Handling Task ---
    let logs_for_tokio                  = Arc::clone(&logs);
    let join_logs_for_tokio             = Arc::clone(&join_logs);
    let sound_channels_for_tokio = Arc::clone(&sound_channels);
    let notification_channels_for_tokio = Arc::clone(&notification_channels);
    let msg_logging_for_tokio           = Arc::clone(&msg_logging_channels);
    let reconnect_times_for_tokio       = Arc::clone(&reconnect_times);
    let user_ids_for_tokio = Arc::clone(&user_ids);


    let join_handle = tokio::spawn(async move {
        tokio::select! {
            _ = async {
                while let Some(message) = incoming_messages.recv().await {
                    let time_str = Local::now().format("%H:%M:%S").to_string();
                    match message {
                        ServerMessage::Privmsg(msg) => {
                            handle_privmsg(
                                &time_str,
                                msg,
                                &logs_for_tokio,
                                &sound_channels_for_tokio,
                                &notification_channels_for_tokio, // This is now a HashMap
                                &msg_logging_for_tokio,
                                &user_ids_for_tokio,
                            );
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

                        // --- NEW: Handle Reconnect ---
                        ServerMessage::Reconnect(_msg) => {
                            let reconnect_msg = format!("{} [RECONNECT] Server requested reconnect. Potential message loss.", time_str);
                            println!("\n{}\n", reconnect_msg.red().bold());

                            // 1. Store the time
                            reconnect_times_for_tokio.lock().unwrap().push(time_str.clone());

                            // 2. Log to all active channels
                            let mut logs = logs_for_tokio.lock().unwrap();
                            for channel_logs in logs.values_mut() {
                                channel_logs.push(format!("{}\n", reconnect_msg));
                            }

                            // 3. Send notification (if not in server mode)
                            send_desktop_notification(
                                "Twitch Client Reconnecting",
                                "The server requested a reconnect. Message logging might be briefly interrupted.",
                                Some(15000),
                                Urgency::Normal
                            );

                            // 4. Reprint prompt
                            print!(">> ");
                            io::stdout().flush().unwrap();
                        }

                        ServerMessage::RoomState(_msg) =>{println!("ROOMSTATE: {}",_msg.channel_login)}

                        ServerMessage::Notice(msg) => {
                            println!("{}[{}][NOTICE] {}", time_str.dimmed(), msg.channel_login.unwrap_or("unknown".to_string()),msg.message_text);
                        }

                        ServerMessage::ClearChat(msg) => {
                            match &msg.action {
                                // --- HANDLE BAN ---
                                ClearChatAction::UserBanned { user_login, user_id, .. } => {
                                    // 1. Store the ID in the global map
                                    // We lock the map and insert the ID immediately
                                    if let Ok(mut ids) = user_ids_for_tokio.lock() {
                                        ids.insert(user_login.clone(), user_id.clone());
                                    }

                                    // 2. Format content to include the ID for the log
                                    let content = format!("{} (ID: {})", user_login, user_id);

                                    handle_moderation_event(
                                        &time_str,
                                        "USER_BANNED",
                                        &msg.channel_login,
                                        &content, // Pass the formatted string with ID
                                        owo_colors::Style::new().red().blink(),
                                                            &logs_for_tokio,
                                                            &msg_logging_for_tokio,
                                                            Urgency::Normal
                                    );
                                }

                                // --- HANDLE TIMEOUT (Optional: You can do the same here if you want) ---
                                ClearChatAction::UserTimedOut { user_login, user_id, timeout_length, .. } => {
                                    // Optional: Store ID for timeouts too
                                    if let Ok(mut ids) = user_ids_for_tokio.lock() {
                                        ids.insert(user_login.clone(), user_id.clone());
                                    }

                                    let content = format!(
                                        "{} (ID: {}) ({}s timeout)",
                                                          user_login,
                                                          user_id,
                                                          timeout_length.as_secs()
                                    );

                                    handle_moderation_event(
                                        &time_str,
                                        "TIMEOUT",
                                        &msg.channel_login,
                                        &content,
                                        owo_colors::Style::new().red().blink(),
                                                            &logs_for_tokio,
                                                            &msg_logging_for_tokio,
                                                            Urgency::Low
                                    );
                                }

                                // --- HANDLE CHAT CLEAR ---
                                ClearChatAction::ChatCleared => {
                                    handle_moderation_event(
                                        &time_str,
                                        "CHAT_CLEARED",
                                        &msg.channel_login,
                                        "The chat was cleared by a moderator.",
                                        owo_colors::Style::new().dimmed(),
                                                            &logs_for_tokio,
                                                            &msg_logging_for_tokio,
                                                            Urgency::Normal
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
                                &msg_logging_for_tokio,
                                Urgency::Low
                            );
                        }
                        ServerMessage::UserNotice(msg) => {
                            handle_user_notice(
                                &time_str,
                                &msg,
                                &logs_for_tokio,
                                &user_ids_for_tokio // <--- Pass here
                            );
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

    // --- Autosave Task ---
    let logs_for_autosave = Arc::clone(&logs);
    let join_logs_for_autosave = Arc::clone(&join_logs);
    let titles_for_autosave = Arc::clone(&channel_titles);
    let reconnect_times_for_autosave = Arc::clone(&reconnect_times);
    let user_ids_for_autosave = Arc::clone(&user_ids);

    tokio::spawn(async move {
        // First, we calculate the duration until the next full hour.
        let now = Utc::now().with_timezone(&Berlin);
        let next_hour = (now + chrono::Duration::hours(1))
        .with_minute(0).unwrap()
        .with_second(0).unwrap()
        .with_nanosecond(0).unwrap();

        let duration_to_wait = (next_hour - now).to_std().unwrap_or_else(|_| {
            // Fallback in the unlikely event of a negative duration
            std::time::Duration::from_secs(3600)
        });

        println!(
            "{}",
            format!(
                "[AUTOSAVE] Next save scheduled for {}.",
                next_hour.format("%H:%M:%S")
            ).cyan()
        );

        // Wait until the top of the hour.
        tokio::time::sleep(duration_to_wait).await;

        // Now, create an interval that ticks every hour.
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            // The first tick completes immediately after the initial sleep.
            interval.tick().await;

            // Use \n to ensure the message appears on a new line, separate from user input.
            println!(
                "\n{}",
                "[AUTOSAVE] Saving logs for all channels...".cyan()
            );

            // Call the existing save function for all channels.
            save_logs(
                "ALL",
                &logs_for_autosave,
                &join_logs_for_autosave,
                &titles_for_autosave,
                &reconnect_times_for_autosave,
                &user_ids_for_autosave, // <--- Pass here
                None,
            );

            println!(
                "{}\n>> ", // Reprint the prompt for a better user experience
                "[AUTOSAVE] Save complete. Next save in one hour.".cyan()
            );
            // We need to flush stdout to make sure the prompt appears immediately.
            io::stdout().flush().unwrap();
        }
    });


    // --- User Input Handling Thread ---


    let client_for_thread                = client.clone();
    let logs_for_thread                  = Arc::clone(&logs);
    let titles_for_thread                = Arc::clone(&channel_titles);
    let join_logs_for_thread             = Arc::clone(&join_logs);
    let channels_for_thread              = Arc::clone(&channels);
    let sound_channels_for_thread        = Arc::clone(&sound_channels);
    let notification_channels_for_thread = Arc::clone(&notification_channels);
    let msg_logging_for_thread           = Arc::clone(&msg_logging_channels);
    let reconnect_times_for_thread       = Arc::clone(&reconnect_times);
    let user_ids_for_thread              = Arc::clone(&user_ids);


    //let vips: Vec<String> = CONFIG.vips.keys().cloned().collect();
    let vips: Vec<String> = CONFIG.vips.keys().cloned().collect();



    let handle = std::thread::spawn(move || -> Result<()> {
        let commands = vec![
            "JOIN".into(),
                                    "PART".into(),
                                    "SOUND".into(),
                                    "SAVE".into(),
                                    "NOTIFY".into(),
                                    "MSGLOGGING".into(), // Add the new command
                                    "EXIT".into(),
                                    "RECONNECT".into(),
                                    "PAUSES".into(),
                                    "STATS".into(),
                                    "TITLE".into()
        ];


        let urgencies = vec!["LOW".into(), "NORMAL".into(), "CRITICAL".into()];

        let completer = CommandCompleter {
            commands: commands.clone(),

                                    urgencies: urgencies.clone(),
                                    joined_channels: Arc::clone(&channels_for_thread),
                                    vips: vips.clone(),
                                    log_channels: Arc::clone(&logs_for_thread),
        };

        let mut rl = Editor::<CommandCompleter, DefaultHistory>::new()?;
        rl.set_helper(Some(completer));

        println!("Commands: JOIN, PART, SOUND, SAVE, TITLE, MSGLOGGING, EXIT");

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
                                // Add this line to enable message logging by default for the new channel
                                msg_logging_for_thread.lock().unwrap().insert(channel.clone());
                                get_or_assign_channel_color(&channel);
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
                            if sound::IS_SOUND_DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
                                println!("{}", "⚠️ Sound is globally disabled (--quiet). Restart to enable.".red());
                                continue;
                            }

                            if let Some(channel) = arg {
                                let mut sound_chans = sound_channels_for_thread.lock().unwrap();

                                if sound_chans.contains_key(&channel) {
                                    sound_chans.remove(&channel);
                                    println!("Sound OFF for {}", channel.yellow());
                                } else {
                                    // LOGIC: Check explicit argument OR fallback to VIP/Low
                                    let specific_urgency = if parts.len() > 2 {
                                        match parts[2].to_uppercase().as_str() {
                                            "LOW" => Some(Urgency::Low),
                                    "NORMAL" => Some(Urgency::Normal),
                                    "CRITICAL" => Some(Urgency::Critical),
                                    _ => None,
                                        }
                                    } else { None };

                                    let urgency = specific_urgency.unwrap_or_else(|| {
                                        if CONFIG.vips.contains_key(&channel) { Urgency::Normal } else { Urgency::Low }
                                    });

                                    sound_chans.insert(channel.clone(), urgency); // Insert with urgency
                                    notification_channels_for_thread.lock().unwrap().remove(&channel);
                                    println!("Sound ON for {} (Level: {:?})", channel.green(), urgency);
                                }
                            }
                        },
                        "NOTIFY" => {
                            // Syntax: NOTIFY <channel> [urgency]
                            if let Some(channel) = arg {
                                let mut notify_chans = notification_channels_for_thread.lock().unwrap();

                                // Check for optional 2nd argument (urgency)
                                let specific_urgency = if parts.len() > 2 {
                                    match parts[2].to_uppercase().as_str() {
                                        "LOW" => Some(Urgency::Low),
                                    "NORMAL" => Some(Urgency::Normal),
                                    "CRITICAL" => Some(Urgency::Critical),
                                    _ => {
                                        println!("Unknown urgency '{}'. Using default.", parts[2]);
                                        None
                                    }
                                    }
                                } else {
                                    None
                                };

                                // Calculate the final urgency to set
                                let target_urgency = specific_urgency.unwrap_or_else(|| {
                                    // Default rule: VIP=Normal, others=Low
                                    if CONFIG.vips.contains_key(&channel) { Urgency::Normal } else { Urgency::Low }
                                });

                                if notify_chans.contains_key(&channel) && parts.len() == 2 {
                                    // If toggling OFF (no urgency arg provided and already exists)
                                    notify_chans.remove(&channel);
                                    println!("Notifications OFF for {}", channel.yellow());
                                } else {
                                    // Turning ON or Updating Urgency
                                    notify_chans.insert(channel.clone(), target_urgency);

                                    // Ensure sound is off
                                    sound_channels_for_thread.lock().unwrap().remove(&channel);

                                    println!("Notifications ON for {} (Level: {:?}) (Sound is OFF)", channel.cyan(), target_urgency);
                                }
                            }
                        },
                        // New command handler
                        "MSGLOGGING" => {
                            if let Some(channel) = arg {
                                let mut logging_chans = msg_logging_for_thread.lock().unwrap();
                                if logging_chans.contains(&channel) {
                                    logging_chans.remove(&channel);
                                    println!("Message console logging OFF for {}", channel.yellow());
                                } else {
                                    logging_chans.insert(channel.clone());
                                    println!("Message console logging ON for {}", channel.green());
                                }
                            } else {
                                println!("Usage: MSGLOGGING <channel>");
                            }
                        },
                        "TITLE" => {
                            if parts.len() < 3 {
                                println!("Usage: TITLE <channel> <stream title...>");
                                continue;
                            }
                            let channel_to_title = parts[1].to_string();
                            let stream_title = parts[2..].join(" ");

                            // Optional: Check if we are even in that channel
                            let joined_channels = channels_for_thread.lock().unwrap();
                            if !joined_channels.iter().any(|c| c.eq_ignore_ascii_case(&channel_to_title)) {
                                println!("⚠️ You are not in channel '{}'. Title not set.", channel_to_title.red());
                                continue;
                            }

                            let mut titles = titles_for_thread.lock().unwrap();
                            titles.insert(channel_to_title.clone(), stream_title.clone());
                            println!(
                                "Set title for '{}' to: {}",
                                channel_to_title.cyan(),
                                     stream_title.green()
                            );
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
                                    &titles_for_thread,
                                    &reconnect_times_for_thread,
                                    &user_ids_for_thread, // <--- Pass here
                                    custom_name.as_deref()
                                );
                            } else {
                                println!("Usage: SAVE <channel|ALL> [optional_custom_name]");
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
        //let _ = exit_tx.send(()); // This signals the async task to stop

        Ok(())
    });
    //let handle_result = handle.join().expect("Input thread panicked");
    if let Err(e) = join_handle.await {
        eprintln!("Error in connection task: {}", e);
    }

    // OPTIONAL: Now that the network is done, you can safely join the input thread
    // just to ensure it cleaned up properly (though it's likely already done).
    let _ = handle.join();

    //join_handle.await?;

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
        // ServerMessage::Reconnect(_) => "RECONNECT", // <-- REMOVED: Now handled explicitly
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
    sound_channels: &Arc<Mutex<HashMap<String, Urgency>>>,
    notification_channels: &Arc<Mutex<HashMap<String, Urgency>>>,
    msg_logging_channels: &Arc<Mutex<HashSet<String>>>,
    user_ids: &Arc<Mutex<HashMap<String, String>>>,
) {

    let username = msg.sender.login.clone();
    let uid = msg.sender.id.clone();

    if let Ok(mut ids) = user_ids.lock() {
        ids.insert(username, uid);
    }

    // Use vips for colorized printing
    let info = CONFIG.vips.get(&msg.channel_login);
    let channel_display = if let Some(color_info) = info.and_then(|c| c.color.as_deref()) {
        // If a color is specified in the config, use it.
        apply_named_color(&msg.channel_login, Some(color_info))
    } else {
        // Otherwise, generate a consistent random color for the channel.
        let (r, g, b) = get_or_assign_channel_color(&msg.channel_login);
        msg.channel_login.truecolor(r, g, b).to_string()
    };

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

    // File logging is unaffected and always happens.
    logs.lock().unwrap().entry(msg.channel_login.clone()).or_default().push(log_line);

    // --- END OF BADGE LOGIC ---

    // Only print to console if the channel is in the logging set.
    if msg_logging_channels.lock().unwrap().contains(&msg.channel_login) {

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
    }
    //notify

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


    if let Some(urgency) = sound_channels.lock().unwrap().get(&msg.channel_login) {
        // Use the urgency stored in the map
        send_desktop_notification(&summary, &body, calculate_timeout(), *urgency);
        play_sound();
    }
    // Check Notify Map (It is now a HashMap<String, Urgency>)
    else if let Some(urgency) = notification_channels.lock().unwrap().get(&msg.channel_login) {
        // Use the urgency stored in the map
        send_desktop_notification(&summary, &body, calculate_timeout(), *urgency);
    }
}

/*https://docs.rs/twitch-irc/latest/twitch_irc/message/enum.UserNoticeEvent.html*/

fn handle_user_notice(
    time: &str,
    msg: &twitch_irc::message::UserNoticeMessage,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    user_ids: &Arc<Mutex<HashMap<String, String>>>,
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
        "{} <{}> [{}] {}\n{}\n",
        time,
        user,
        event_type,
        sys_msg,
        user_msg
    );

    println!(
        "{} [{}][{}] {}: {}\n→ {}\n",
        time.dimmed(),
             channel,
             user,
             event_type.blue(),
             sys_msg.yellow(),
             user_msg
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
    msg_logging_channels: &Arc<Mutex<HashSet<String>>>,
    urgency: Urgency // <--- New Argument
) {
    let log_line = format!("{time_str} {event_type}: [#{channel}] {content}\n");
    println!("{}\n", log_line.style(style));

    let summary = format!("Moderation in #{}", channel);
    let body = format!("[{}] {}\n", event_type, content);

    // Pass the urgency through
    send_desktop_notification(&summary, &body, Some(10000), urgency);

    if msg_logging_channels.lock().unwrap().contains(channel) {
        play_sound();
    }

    let mut logs = log_store.lock().unwrap();
    logs.entry(channel.to_string()).or_default().push(log_line);
}

/*struct JoinEvent {
    channel: String,
    user: String,
    event: String, // "J" or "P"
}*/

fn handle_join_or_part(
    event_type: &str,
    time_str: &str,
    channel: &str,
    username: &str,
    log_store: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    // CHANGED TYPE:
    join_log_store: &Arc<Mutex<Vec<String>>>,
){
    // Shorten event type for the log (J or P)
    let short_event = if event_type == "JOIN" { "J" } else { "P" };

    let msg = format!("{} {} {}\n", channel, short_event, username);
    // Push to global vector
    join_log_store.lock().unwrap().push(msg);

    // VIP Logic (Kept largely the same, but logging to the chat log needs the old format or a copy)
    if CONFIG.vips.contains_key(username) {
        println!("{}\n", format!("*** VIP {username} has {event_type}ed {channel} ***").yellow());

        // We create a standard message for the CHAT log (not the join log)
        let chat_log_msg = format!("{time_str} [{event_type}] {username}\n");

        log_store.lock().unwrap()
        .entry(channel.to_string())
        .or_default()
        .push(chat_log_msg);

        send_desktop_notification(channel, &format!("{} {}ed", username, event_type), Some(7000), Urgency::Normal);

        if event_type == "JOIN" && username != channel {
            play_sound();
        }
    }
}

fn save_logs(
    target: &str,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    join_logs: &Arc<Mutex<Vec<String>>>, // Kept as Vec (Global) to match main.rs
             channel_titles: &Arc<Mutex<HashMap<String, String>>>,
             reconnect_times: &Arc<Mutex<Vec<String>>>,
             user_ids: &Arc<Mutex<HashMap<String, String>>>,
             custom_name: Option<&str>,
) {
    let logs_locked = logs.lock().unwrap();
    let mut join_logs_locked = join_logs.lock().unwrap();
    let titles_locked = channel_titles.lock().unwrap();
    let reconnects_locked = reconnect_times.lock().unwrap();
    let user_ids_locked = user_ids.lock().unwrap();

    // 1. Setup Log Directory
    let mut log_dir_path = PathBuf::from(&*LOG_DIRECTORY);
    if !log_dir_path.exists() {
        if let Err(e) = create_dir_all(&log_dir_path) {
            eprintln!(
                "⚠️ Failed to create log directory '{}': {}. Defaulting to /tmp.",
                log_dir_path.display(),
                      e
            );
            log_dir_path = PathBuf::from("/tmp");
        }
    }

    // --- PART 1: SAVE CHAT MESSAGES (Per Channel) ---
    let targets: Vec<String> = if target.eq_ignore_ascii_case("ALL") {
        logs_locked.keys().cloned().collect()
    } else {
        vec![target.to_string()]
    };

    let reconnect_warning = if reconnects_locked.is_empty() {
        String::new()
    } else {
        let times_str = reconnects_locked.join(", ");
        format!(
            "\n\n⚠️ WARNING: The client reconnected at the following times: [{}].\n   Messages may have been lost during these periods.",
            times_str
        )
    };

    for chan in targets {
        let name_to_use = custom_name.or_else(|| titles_locked.get(&chan).map(|s| s.as_str()));

        if let Some(messages) = logs_locked.get(&chan) {
            // RESTORED FUNCTIONALITY:
            // check if there are actual user messages (containing < and >)
            // If not, we skip saving this file.
            if !messages.iter().any(|line| line.contains('<') && line.contains('>')) {
                println!(
                    "Skipping message log for '{}': No chat messages, only join/part events.",
                    chan.yellow()
                );
                continue;
            }

            // Calculate timestamp based on first message, or now
            let time_part = messages
            .iter()
            .find(|line| line.contains('<') && line.contains('>'))
            .map(|first_line| first_line[0..8].replace(':', "-"))
            .unwrap_or_else(|| Local::now().format("%H-%M-%S").to_string());

            let timestamp_for_save = format!("{}_{}", *STARTUP_DATE, time_part);

            // Generate Filename
            let file_name = if let Some(name) = name_to_use {
                let sanitized_name = name.replace(' ', "_");
                format!("{}_{}_{}.txt", chan, sanitized_name, timestamp_for_save)
            } else {
                format!("{}_msgs_{}.txt", chan, timestamp_for_save)
            };

            let file_path = log_dir_path.join(&file_name);

            // --- Stats Calculation (Restored from Old Code) ---
            let mut msg_count = 0;
            let mut unique_chatters = HashSet::new();
            let mut mod_events = 0;
            let mut sub_events = 0;
            let mut raid_events = 0;
            let mut milestone_events = 0;
            let mut other_events = 0;
            let mut user_message_stats: HashMap<String, usize> = HashMap::new();

            for line in messages {
                // Flag to determine if we should attempt to count this as a user message
                let mut potentially_chat = false;

                if line.contains("SUBORRESUB")
                    || line.contains("SUBGIFT")
                    || line.contains("SUBMYSTERYGIFT")
                    || line.contains("ANONSUBMYSTERYGIFT")
                    || line.contains("GIFTPAIDUPGRADE")
                    || line.contains("ANONPAIDGIFTUPGRADE")
                    || line.contains("PRIMEPAIDUPGRADE")
                    || line.contains("COMMUNITYPAYFORWARD")
                    || line.contains("[SUBSCRIPTION]")
                    || line.contains("[RESUBSCRIPTION]")
                    {
                        sub_events += 1;
                        potentially_chat = true; // Subs often have messages
                    } else if line.contains("USER_BANNED")
                        || line.contains("CLEARMSG")
                        || line.contains("TIMEOUT")
                        || line.contains("CHAT_CLEARED")
                        {
                            mod_events += 1;
                        } else if line.contains("RAID") {
                            raid_events += 1;
                        } else if line.contains("VIEWERMILESTONE") || line.contains("MILESTONE") {
                            milestone_events += 1;
                            potentially_chat = true; // Milestones usually have messages
                        } else if line.contains("ANNOUNCEMENT")
                            || line.contains("SHAREDCHATNOTICE")
                            {
                                other_events += 1;
                                potentially_chat = true; // Announcements are messages
                            } else {
                                // Standard message fallback
                                potentially_chat = true;
                            }

                            // If it's a message type we care about, try to extract user and count it
                            if potentially_chat && line.matches(':').count() >= 2 && line.contains('<') && line.contains('>') {
                                if let Some(start) = line.find('<') {
                                    if let Some(end) = line.find('>') {
                                        if end > start {
                                            let username = &line[start + 1..end];

                                            // Optional: You might want to filter out "Nightbot" or specific bots here if desired,
                                            // but standard logic usually counts them.

                                            msg_count += 1;
                                            unique_chatters.insert(username.to_string());
                                            *user_message_stats.entry(username.to_string()).or_default() += 1;
                                        }
                                    }
                                }
                            }
            }

            // --- Stats Formatting (Median & Activity) ---
            let mut sorted_stats: Vec<_> = user_message_stats.into_iter().collect();
            // Sort by count DESC, then alphabetically
            sorted_stats.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            let user_activity_summary = sorted_stats
            .iter()
            .map(|(user, count)| format!("[{}:{}]", user, count))
            .collect::<Vec<_>>()
            .join(" ");

            let msg_counts: Vec<usize> = sorted_stats.iter().map(|(_, count)| *count).collect();

            let avg_per_chatter = if !msg_counts.is_empty() {
                msg_counts.iter().sum::<usize>() as f64 / msg_counts.len() as f64
            } else {
                0.0
            };

            // Median Calculation (Restored)
            let median_value = if msg_counts.is_empty() {
                None
            } else {
                let mut sorted_counts = msg_counts.clone();
                sorted_counts.sort_unstable(); // Ascending
                let len = sorted_counts.len();
                if len % 2 == 1 {
                    Some(format!("{}", sorted_counts[len / 2]))
                } else {
                    Some(format!(
                        "{}, {}",
                        sorted_counts[len / 2 - 1],
                        sorted_counts[len / 2]
                    ))
                }
            };

            let median_line = match median_value {
                Some(m) => format!("Median value: {}", m),
                None => "Median value: N/A".to_string(),
            };

            // Construct the Header
            let header = format!(
                "--- Message/Event Log ---\n# {} {}\n({} messages from {} chatters, {:.1} messages per chatter)\n({} Banns, Deletions, and Timeouts)\n({} Subs/Giftsubs)\n({} Raids)\n({} Milestones)\n({} Others)\nChatter Activity (msg count): {}\n{}\n{}{}\n\n",
                                 chan,
                                 file_name,
                                 msg_count,
                                 unique_chatters.len(),
                                 avg_per_chatter,
                                 mod_events,
                                 sub_events,
                                 raid_events,
                                 milestone_events,
                                 other_events,
                                 user_activity_summary,
                                 median_line,
                                 "",
                                 reconnect_warning
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

            if let Ok(mut f) = File::create(&file_path) {
                if f.write_all(&content_with_bom).is_ok() {
                    println!("Saved {} messages to {}", messages.len(), file_path.display());
                }
            }
        }
    }

    // --- PART 2: SAVE GLOBAL JOIN/PART LOGS ---
    // Note: We use the Global Vec (current main.rs architecture)
    if !join_logs_locked.is_empty() {
        let save_timestamp = Local::now().format("%d.%m.%Y %H:%M:%S").to_string();

        // We use SESSION_TIMESTAMP for the filename so checks are appended to one file
        let filename = format!("JOIN_PART_LOG_{}.txt", *SESSION_TIMESTAMP);
        let join_file_path = log_dir_path.join(filename);

        let file_chunk = format!(
            "\n--- [SAVED AT {}] ---\n{}",
            save_timestamp,
            join_logs_locked.join("")
        );

        let write_result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&join_file_path)
        .and_then(|mut file| file.write_all(file_chunk.as_bytes()));

        match write_result {
            Ok(_) => {
                println!(
                    "Appended {} global events to {}",
                    join_logs_locked.len(),
                         join_file_path.display()
                );
                join_logs_locked.clear();
            }
            Err(e) => {
                eprintln!("⚠️ Error appending to global log file: {}", e);
            }
        }
    }

    // --- PART 3: SAVE USER IDs ---
    if !user_ids_locked.is_empty() {
        // We use the same timestamp format
        let filename = format!("USER_IDS_{}.txt", *STARTUP_DATE);
        let id_file_path = log_dir_path.join(filename);


        let mut file_content = String::new();
        for (name, id) in user_ids_locked.iter() {
            file_content.push_str(&format!("{}: {}\n", name, id));
        }

        // Write to file (Overwrite is safer to ensure the list is always complete and deduped based on current memory)
        if let Ok(mut f) = File::create(&id_file_path) {
            let _ = f.write_all(file_content.as_bytes());
            println!("Saved {} User IDs to {}", user_ids_locked.len(), id_file_path.display());
        }
    }
}

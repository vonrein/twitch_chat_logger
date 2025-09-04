use rustyline::Editor;
use rustyline::history::DefaultHistory;

mod completer;
use completer::CommandCompleter;

use anyhow::Result;
use chrono::Local;
use clap::Parser;
use once_cell::sync::Lazy;
use owo_colors::OwoColorize;
use rodio::{Decoder, OutputStream, Sink};
use rustyline::error::ReadlineError;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, Mutex};
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::{PrivmsgMessage, ServerMessage};
use twitch_irc::message::ClearChatAction;
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};
use chrono::prelude::*;
use chrono_tz::Europe::Berlin;



// --- Constants ---



const SOUND_PATH: &str = "/home/steve/twitchStatistik/nesCapture.mp3";

static DEFAULT_CHANNELS: Lazy<HashSet<String>> = Lazy::new(|| {
    vec![
        "janistantv", "shellingford92", "coder2k", "gyrosgeier", "cirdan77", "markomonxd",
    ]
    .into_iter()
    .map(String::from)
    .collect()
});

static VIPS: Lazy<HashSet<String>> = Lazy::new(|| {
    vec![
        "janistantv", "shellingford92", "coder2k", "gyrosgeier", "cirdan77", "markomonxd",
        "jvpeek", "gronkh", "thebrutzler", "codingpurpurtentakel", "deke64", "dysifu",
        "edelive", "eisenfutzi", "gmrasmussvane", "katharinareinecke", "kdrkitten",
        "meli070", "nerdyolli", "graefinzahl", "sempervideo", "nini_jay", "bonjwa",
        "lixoulive", "lcolonq", "tyeni15", "wildmics",
    ]
    .into_iter()
    .map(String::from)
    .collect()
});

// --- Command-Line Argument Parser ---
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// List of Twitch channels to join
    #[arg(name = "CHANNELS")]
    channels: Vec<String>,
}

// --- Sound Player ---
fn play_sound() {
    std::thread::spawn(|| {
        if let Ok((stream, stream_handle)) = OutputStream::try_default() {
            if let Ok(sink) = Sink::try_new(&stream_handle) {
                if let Ok(file) = File::open(SOUND_PATH) {
                    if let Ok(source) = Decoder::new(BufReader::new(file)) {
                        sink.append(source);
                        sink.sleep_until_end(); // block inside thread only
                    } else {
                        eprintln!("Error: Failed to decode sound file.");
                    }
                } else {
                    eprintln!("Error: Could not open sound file at '{}'", SOUND_PATH);
                }
            }
            drop(stream); // drop after playing
        } else {
            eprintln!("Error: Could not find default audio output device.");
        }
    });
}

fn get_local_timestamp() -> String {
    let now = Utc::now().with_timezone(&Berlin);
    format!(
        "{:02}_{:02}_{:04}_{:02}-{:02}-{:02}",
        now.day(),
            now.month(),
            now.year(),
            now.hour(),
            now.minute(),
            now.second()
    )
}

// --- Main Application Logic ---
#[tokio::main]
async fn main() -> Result<()> {
    use tokio::sync::oneshot;
    let cli = Cli::parse();
    //let (exit_tx, exit_rx) = oneshot::channel();
    let (exit_tx, exit_rx) = oneshot::channel::<()>();

    let initial_channels: Vec<String> = if cli.channels.is_empty() {
        DEFAULT_CHANNELS.iter().cloned().collect()
    } else {
        cli.channels
    };

    let config = ClientConfig::default();
    let (mut incoming_messages, client) =
    TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);

    // --- Shared State ---
    let channels = Arc::new(Mutex::new(initial_channels.clone()));
    let logs = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    //  let join_logs = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    let sound_channels = Arc::new(Mutex::new(
        initial_channels.iter().cloned().collect::<HashSet<String>>(),
    ));

    // --- Join Initial Channels ---
    for channel in &initial_channels {
        client.join(channel.clone())?;
        println!("Joined initial channel: {}", channel.green());
    }

    // --- Message Handling Task ---
    let logs_for_tokio = Arc::clone(&logs);
    // let join_logs_for_tokio = Arc::clone(&join_logs);


    let sound_channels_for_tokio = Arc::clone(&sound_channels);

    let join_handle = tokio::spawn(async move {
        tokio::select! {
            _ = async {
                while let Some(message) = incoming_messages.recv().await {
                    let time_str = Local::now().format("%H:%M:%S").to_string();
                    match message {
                        ServerMessage::Privmsg(msg) => {
                            handle_privmsg(&time_str, msg, &logs_for_tokio, &sound_channels_for_tokio);
                        }

                        ServerMessage::Join(_msg) => {
                            println!("JOIN");
                            //handle_join_or_part("JOIN", &time_str, &msg.channel_login, &msg.user_login, &logs_for_tokio, &join_logs_for_tokio);

                        }
                        ServerMessage::Part(_msg) => {
                            println!("PART");
                            //handle_join_or_part("PART", &time_str, &msg.channel_login, &msg.user_login, &logs_for_tokio, &join_logs_for_tokio);
                        }
                        ServerMessage::Ping(_msg) => {
                            println!("PING");
                        }
                        ServerMessage::Pong(_msg) => {
                            println!("PONG");
                        }
                        ServerMessage::RoomState(_msg) =>{}

                        ServerMessage::Notice(msg) => {
                            println!("{}[{}][NOTICE] {}", time_str.dimmed(), msg.channel_login.unwrap_or("unknown".to_string()),msg.message_text);
                        }

                        ServerMessage::ClearChat(msg) => {
                            if let ClearChatAction::UserBanned { user_login, .. } = &msg.action {
                                handle_moderation_event(
                                    &time_str,
                                    "USER_BANNED",
                                    &msg.channel_login,
                                    user_login,
                                    owo_colors::Style::new().red().blink(),
                                                        &logs_for_tokio,
                                );
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
    // let join_logs_for_thread = Arc::clone(&join_logs);

    let vips: Vec<String> = VIPS.iter().cloned().collect();

    let msg_logs: Vec<String> = {
        let logs_guard = logs.lock().unwrap();
        logs_guard.keys().cloned().collect()
    };

    let channels_for_thread = Arc::clone(&channels);
    let sound_channels_for_thread = Arc::clone(&sound_channels);

    let handle = std::thread::spawn(move || -> Result<()> {

        let commands = vec![
            "JOIN".into(),
                                    "PART".into(),
                                    "SOUND".into(),
                                    "SAVE".into(),
                                    "EXIT".into(),
                                    "RECONNECT".into(),
                                    "PAUSES".into(),
                                    "STATS".into(),
        ];


        //todo

        let mut args_map = HashMap::new();
        args_map.insert("JOIN".into(), vips.clone());
        args_map.insert("PART".into(), msg_logs.clone());
        args_map.insert("SOUND".into(), {
            let mut combined = vips.clone();
            combined.extend(msg_logs.clone());
            combined.sort();
            combined.dedup();
            combined
        });
        args_map.insert("SAVE".into(), msg_logs.clone());


        let completer = CommandCompleter { commands: commands.clone(), args_map };
        //let mut rl = Editor::<(), DefaultHistory>::new().unwrap();
        /*
         *        let mut rl = Editor::<CommandCompleter, DefaultHistory>::new()?;
         *        rl.set_completer(Some(completer));
         */
        let mut rl = Editor::<CommandCompleter, DefaultHistory>::new()?;
        rl.set_helper(Some(completer));

        println!("Commands: JOIN/PART <channel>, SOUND <channel>, SAVE <channel|ALL>, EXIT");

        loop {
            let line = rl.readline(">> ");
            match line {
                Ok(input) => {
                    let words: Vec<&str> = input.trim().split_whitespace().collect();
                    match words.as_slice() {
                        ["EXIT"] => {
                            println!("Shutting down...");
                            let joined_channels = channels_for_thread.lock().unwrap().clone();
                            for channel in joined_channels {
                                let _ = client_for_thread.part(channel.clone());
                                println!("Left channel: {}", channel);
                            }

                            let _ = exit_tx.send(()); // notify the async task
                            break;
                        }
                        ["JOIN", chan] => println!("Joining {}", chan.blue()),
                                    ["PART", chan] => println!("Parting {}", chan.red()),
                                    ["SOUND", chan] => println!("Playing sound for {}", chan.cyan()),
                                    ["SAVE", chan] => println!("Saving logs for {}", chan.cyan()),
                                    _ => println!("{}: '{}'", "Unknown command".red(), input.trim()),
                    }
                }
                Err(_) => break,
            }
        }

        Ok(())
    });
    let handle_result = handle.join().expect("Input thread panicked");
    if let Err(e) = handle_result {
        eprintln!("Error in input thread: {:?}", e);
    }

    // Wait for the message task to complete (usually only after exit signal)
    join_handle.await?;

    Ok(())
}
/*
 *        loop {
 *            match rl.readline("> ") {
 *                Ok(line) => {
 *                    let _ = rl.add_history_entry(line.as_str());
 *                    let parts: Vec<&str> = line.trim().split_whitespace().collect();
 *                    if parts.is_empty() {
 *                        continue;
 *                    }
 *
 *                    let cmd = parts[0].to_uppercase();
 *                    let arg = parts.get(1).map(|s| s.to_string());
 *
 *                    match cmd.as_str() {
 *                        "JOIN" => {
 *                            if let Some(channel) = arg {
 *                                let _ = client_for_thread.join(channel.clone());
 *                                channels_for_thread.lock().unwrap().push(channel);
 *                            }
 *                        }
 *                        "PART" => {
 *                            if let Some(channel) = arg {
 *                                let _ = client_for_thread.part(channel.clone());
 *                                channels_for_thread.lock().unwrap().retain(|c| c != &channel);
 *                            }
 *                        }
 *                        "SOUND" => {
 *                            if let Some(channel) = arg {
 *                                let mut sound_chans = sound_channels_for_thread.lock().unwrap();
 *                                if sound_chans.contains(&channel) {
 *                                    sound_chans.remove(&channel);
 *                                    println!("Sound OFF for {}", channel.yellow());
 *                                } else {
 *                                    sound_chans.insert(channel.clone());
 *                                    println!("Sound ON for {}", channel.green());
 *                                }
 *                            }
 *                        }
 *                        "SAVE" => {
 *                            if let Some(target) = arg {
 *                                save_logs(&target, &logs_for_thread);
 *                            }
 *                        }
 *
 *                        "EXIT" => {
 *                            println!("Shutting down...");
 *                            let joined_channels = channels_for_thread.lock().unwrap().clone();
 *                            for channel in joined_channels {
 *                                let _ = client_for_thread.part(channel.clone());
 *                                println!("Left channel: {}", channel);
 *                            }
 *
 *                            let _ = exit_tx.send(()); // notify the async task
 *                            break;
 *                        }
 *                        _ => println!("Unknown command. Try: JOIN, PART, SOUND, SAVE, EXIT"),
 *                    }
 *                }
 *                Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
 *                    println!("Exiting...");
 *                    break;
 *                }
 *                Err(err) => {
 *                    println!("Input Error: {:?}", err);
 *                    break;
 *                }
 *            }
 *        }
 *    });
 *
 *    handle.join().unwrap();
 *    join_handle.await?;
 *    Ok(())
 * }*/

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
        _ => "OTHER",
    };

    if kind == "OTHER" {
        println!("{} [SYSTEM: OTHER] {:?}", time.dimmed(), message);
    } else {
        println!("{} [SYSTEM: {}]", time.dimmed(), kind);
    }
}

fn handle_privmsg(
    time_str: &str,
    msg: PrivmsgMessage,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    sound_channels: &Arc<Mutex<HashSet<String>>>,
) {

    let mut custom_badges = msg.badges.iter()
    .map(|b| format!("{}/{}", b.name, b.version))
    .collect::<Vec<_>>();

    let tags = &msg.source.tags;

    // Add virtual badges based on tag fields
    if let Some(first_msg) = tags.0.get("first-msg").and_then(|v| v.as_deref()) {
        if first_msg == "1" {
            custom_badges.push("[FIRSTMSG]".to_string());
        }
    }

    if let Some(returning) = tags.0.get("returning-chatter").and_then(|v| v.as_deref()) {
        if returning == "1" {
            custom_badges.push("[RETURNING]".to_string());
        }
    }
    /*
     *   // Other interesting info
     *   for &key in &["mod", "subscriber"] {
     *       if let Some(val) = tags.0.get(key).and_then(|v| v.as_deref()) {
     *           if val == "1" {
     *               custom_badges.push(format!("{}/1", key));
}
}
}
*/
    let badges_for_log = custom_badges.join(",");
    let badge_info_for_console = if !custom_badges.is_empty() {
        format!("[{}] ", custom_badges.join(", ").yellow())
    } else {
        String::new()
    };

    let log_line = format!(
        "{} [{}] <{}> {}",
        time_str, badges_for_log, msg.sender.name, msg.message_text
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
             msg.channel_login.cyan().bold(),
             user_styled.bold(),
             badge_info_for_console, // Badges are printed here
             msg.message_text
    );

    if sound_channels.lock().unwrap().contains(&msg.channel_login) {
        play_sound();
    }
}

fn handle_user_notice(
    time: &str,
    msg: &twitch_irc::message::UserNoticeMessage,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
) {
    use owo_colors::OwoColorize;

    let event_type = format!("{:?}", msg.event); // More readable than UUID
    let channel = &msg.channel_login;
    let user = &msg.sender.name;
    let user_msg = msg.message_text.as_deref().unwrap_or("");
    let sys_msg = msg.system_message.trim();

    // Compose log line
    let line = format!(
        "{} {}[SYSTEM {}] <{}> {}\n→ {}",
        time.dimmed(),
                       channel,
                       event_type.to_uppercase().blue(),
                       user,
                       user_msg,
                       sys_msg.yellow()
    );

    println!("{}", line);

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

    let mut logs = log_store.lock().unwrap();
    logs.entry(channel.to_string()).or_default().push(log_line);
}


/*
 * fn handle_join_or_part(
 *    event_type: &str,
 *    time_str: &str,
 *    channel: &str,
 *    username: &str,
 *    log_store: &Arc<Mutex<HashMap<String, Vec<String>>>>,
 *    join_log_store: &Arc<Mutex<HashMap<String, Vec<String>>>>,
 * ){
 *
 *    let msg = format!("{time_str} [{event_type}] {username} in #{channel}");
 *    join_log_store.lock().unwrap()
 *    .entry(channel.to_string())
 *    .or_default()
 *    .push(msg.clone());
 *
 *    if VIPS.contains(username)  {
 *        println!("{}", format!("*** VIP {username} has {event_type}ed {channel} ***").yellow());
 *
 *
 *        // Save to both general and join logs
 *
 *        log_store.lock().unwrap()
 *        .entry(channel.to_string())
 *        .or_default()
 *        .push(msg.clone());
 *
 *
 *        if event_type == "JOIN" && username != channel {
 *            play_sound();
 *        }
 *    }
 *
 * }*/



// --- Utility Functions ---
fn save_logs(
    target: &str,
    logs: &Arc<Mutex<HashMap<String, Vec<String>>>>,
) {
    let logs_locked = logs.lock().unwrap();
    // let join_logs_locked = join_logs.lock().unwrap();

    let targets: Vec<String> = if target.eq_ignore_ascii_case("ALL") {
        logs_locked.keys().cloned().collect()
    } else {
        vec![target.to_string()]
    };
    let timestamp = get_local_timestamp();

    for chan in targets {
        if let Some(messages) = logs_locked.get(&chan) {
            let file = format!("/tmp/{}_log_{}.txt", chan, timestamp);
            let numbered = messages
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{}. {}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

            if std::fs::write(&file, numbered).is_ok() {
                println!("Saved {} messages to {}", messages.len(), file);
            }
        }

        /*  if let Some(join_msgs) = join_logs_locked.get(&chan) {
         *            let file = format!("/tmp/{}_joins_{}.txt", chan, timestamp);
         *            if std::fs::write(&file, join_msgs.join("\n")).is_ok() {
         *                println!("Saved {} JOIN/PART events to {}", join_msgs.len(), file);
    }
    }*/
    }
}

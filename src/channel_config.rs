use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use anyhow::{Result, anyhow};
use owo_colors::OwoColorize;

#[derive(Debug)]
pub struct ChannelInfo {
    pub color: Option<String>, // Optional named color
}

#[derive(Debug)]
pub struct ChannelConfig {
    pub default_channels: Vec<String>,
    pub vips: HashMap<String, ChannelInfo>,
}

/// Load channel configuration from file.
/// First line = number of default channels (N).
/// Next N lines = default channels (also VIPs).
/// Remaining lines = additional VIPs.
pub fn load_channel_config(path: &str) -> Result<ChannelConfig> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file).lines().filter_map(Result::ok);

    let default_count: usize = reader
    .next()
    .ok_or_else(|| anyhow!("Missing first line for default count"))?
    .trim()
    .parse()
    .map_err(|e| anyhow!("Invalid number on first line: {e}"))?;

    let mut default_channels = Vec::new();
    let mut vips = HashMap::new();

    for (i, line) in reader.enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.splitn(2, ':');
        let name = parts.next().unwrap().trim().to_string();
        let color = parts.next().map(|c| c.trim().to_string());

        if i < default_count {
            default_channels.push(name.clone());
        }

        vips.insert(name, ChannelInfo { color });
    }

    Ok(ChannelConfig {
        default_channels,
       vips,
    })
}

/// Apply a named color to a string using owo-colors.
/// Falls back to cyan if unknown or not provided.

pub fn apply_named_color(text: &str, color_name: Option<&str>) -> String {
    match color_name.map(str::to_lowercase).as_deref() {
        Some("red") => format!("{}", text.red().bold()),
        Some("green") => format!("{}", text.green().bold()),
        Some("blue") => format!("{}", text.blue().bold()),
        Some("yellow") => format!("{}", text.yellow().bold()),
        Some("magenta") => format!("{}", text.magenta().bold()),
        Some("cyan") => format!("{}", text.cyan().bold()),
        Some("white") => format!("{}", text.white().bold()),
        Some("black") => format!("{}", text.black().bold()),
        Some(hex) if hex.starts_with('#') && hex.len() == 7 => {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[1..3], 16),
                                            u8::from_str_radix(&hex[3..5], 16),
                                            u8::from_str_radix(&hex[5..7], 16),
            ) {
                return format!("{}", text.truecolor(r, g, b).bold());
            }
            format!("{}", text.cyan().bold()) // fallback for invalid hex
        }
        _ => format!("{}", text.cyan().bold()), // fallback for None or unrecognized color
    }
}


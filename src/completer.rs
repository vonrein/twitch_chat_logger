use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{Validator, ValidationContext, ValidationResult};
use rustyline::{Context, Helper};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The completer now holds shared references to the application's dynamic state.
pub struct CommandCompleter {
    pub commands: Vec<String>,
    pub joined_channels: Arc<Mutex<Vec<String>>>,
    pub vips: Vec<String>,
    pub log_channels: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl Completer for CommandCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), rustyline::error::ReadlineError> {
        let (start, completions) = self.dynamic_complete(line);
        let pairs: Vec<Pair> = completions
        .into_iter()
        .map(|comp| Pair {
            display: comp.clone(),
             replacement: comp,
        })
        .collect();
        Ok((start, pairs))
    }
}

impl CommandCompleter {
    /// Generates completion suggestions dynamically based on the current application state.
    pub fn dynamic_complete(&self, line: &str) -> (usize, Vec<String>) {
        let start_of_content = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        let words: Vec<&str> = trimmed.split_whitespace().collect();

        // Block completions if three or more words are already typed
        let word_count = words.len() + if line.ends_with(' ') { 1 } else { 0 };
        if word_count >= 3 {
            return (line.len(), vec![]);
        }

        if words.is_empty() {
            return (0, self.commands.clone());
        }

        // Case 1: User is typing the command name.
        if words.len() == 1 && !line.ends_with(' ') {
            let matches: Vec<String> = self.commands
            .iter()
            .filter(|cmd| cmd.starts_with(&words[0].to_uppercase()))
            .cloned()
            .collect();
            return (start_of_content, matches);
        }

        // Case 2: User is typing an argument for a command.
        let command = words[0].to_uppercase();

        let potential_args = match command.as_str() {
            "PART" => self.joined_channels.lock().unwrap().clone(),
            "JOIN" => self.vips.clone(),
            "SOUND" | "NOTIFY" => {
                let log_keys: Vec<String> = self.log_channels.lock().unwrap().keys().cloned().collect();
                let mut combined = self.joined_channels.lock().unwrap().clone();
                combined.extend(log_keys);
                combined.extend(self.vips.clone());
                combined.sort_unstable();
                combined.dedup();
                combined
                /* //before gemini change
                let log_keys: Vec<String> = self.log_channels.lock().unwrap().keys().cloned().collect();
                let mut combined = log_keys;
                combined.sort();
                combined.extend(self.vips.clone());

                combined.dedup();
                combined
                */
            }
            "SAVE" => self.log_channels.lock().unwrap().keys().cloned().collect(),
            _ => Vec::new(),
        };

        if potential_args.is_empty() {
            return (line.len(), vec![]);
        }

        let arg_fragment = if line.ends_with(' ') { "" } else { words.last().unwrap_or(&"") };

        let matches: Vec<String> = potential_args
        .into_iter()
        .filter(|arg| arg.to_lowercase().starts_with(&arg_fragment.to_lowercase()))
        .collect();

        let start_of_last_word = line.rfind(char::is_whitespace).map_or(start_of_content, |i| i + 1);

        if line.ends_with(' ') {
            (line.len(), matches)
        } else {
            (start_of_last_word, matches)
        }
    }
}

impl Hinter for CommandCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        if pos < line.len() {
            return None;
        }

        let (start, suggestions) = self.dynamic_complete(line);
        if let Some(first_suggestion) = suggestions.first() {
            let fragment_to_complete = &line[start..];
            if first_suggestion.to_lowercase().starts_with(&fragment_to_complete.to_lowercase()) {
                return Some(first_suggestion[fragment_to_complete.len()..].to_string());
            }
        }
        None
    }
}

impl Highlighter for CommandCompleter {}
impl Validator for CommandCompleter {
    fn validate(&self, _: &mut ValidationContext) -> Result<ValidationResult, rustyline::error::ReadlineError> {
        Ok(ValidationResult::Valid(None))
    }
}

impl Helper for CommandCompleter {}

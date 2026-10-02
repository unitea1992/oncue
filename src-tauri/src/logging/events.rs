use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub category: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogEvent {
    pub fn info(category: &str, message: &str) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Info,
            category: category.to_string(),
            message: message.to_string(),
            details: None,
        }
    }

    pub fn error(category: &str, message: &str) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Error,
            category: category.to_string(),
            message: message.to_string(),
            details: None,
        }
    }

    pub fn warn(category: &str, message: &str) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Warn,
            category: category.to_string(),
            message: message.to_string(),
            details: None,
        }
    }

    pub fn debug(category: &str, message: &str) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Debug,
            category: category.to_string(),
            message: message.to_string(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

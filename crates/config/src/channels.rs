#[derive(Debug, Clone)]
pub struct ExternalChannelConfig {
    pub name: String,
    pub kind: ExternalChannelKind,
}

#[derive(Debug, Clone)]
pub enum ExternalChannelKind {
    Ntfy {
        url: String,
        topic: String,
        priority: Option<String>,
    },
    Webhook {
        url: String,
        method: Option<String>,
        headers: Vec<(String, String)>,
    },
    Macos {
        default_priority: Option<String>,
        throttle_window_secs: Option<u64>,
        sound: Option<bool>,
        app_name: Option<String>,
        web_url: Option<String>,
    },
    Windows {
        throttle_window_secs: Option<u64>,
        sound: Option<bool>,
        app_name: Option<String>,
        app_id: Option<String>,
    },
}

use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadSignal {
    Agent,
    Hub,
    Workspace,
}

pub type ReloadSender = mpsc::UnboundedSender<ReloadSignal>;

pub type ReloadReceiver = mpsc::UnboundedReceiver<ReloadSignal>;

pub struct ServerCommand {
    pub name: String,
    pub args: Option<String>,
    pub reply_tx: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
}

pub struct StopRequest {
    pub reply_to: Option<String>,
    pub result_tx: Option<tokio::sync::oneshot::Sender<bool>>,
}

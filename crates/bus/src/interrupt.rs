use tokio::sync::mpsc;

use crate::AgentMessageEvent;
use crate::MessageEvent;

#[derive(Clone)]
pub enum Interrupt {
    UserMessage(MessageEvent),
    AgentMessage(AgentMessageEvent),
    Subconscious(String),
    Stopped,
}

pub trait InterruptSource: Send {
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError>;
}

impl InterruptSource for mpsc::Receiver<Interrupt> {
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError> {
        mpsc::Receiver::try_recv(self)
    }
}

impl InterruptSource for mpsc::UnboundedReceiver<Interrupt> {
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError> {
        mpsc::UnboundedReceiver::try_recv(self)
    }
}

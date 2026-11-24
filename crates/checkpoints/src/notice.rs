use residuum_bus::Publisher;

pub(super) async fn publish(publisher: Option<&Publisher>, message: String) {
    if let Some(publisher) = publisher {
        residuum_bus::notice::publish_notice(publisher, message).await;
    }
}

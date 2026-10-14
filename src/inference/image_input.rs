//! Keeping images away from models that can't take them.
//!
//! Every provider a chain builds is wrapped in an [`ImageInputGuard`], so the
//! rewrite happens once, before any provider-specific conversion, for every
//! provider and every failover hop alike. The conversation itself is never
//! changed: only the outgoing request is, so switching back to a model that
//! takes images sends them again.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;

use super::stream::TrackedSink;
use super::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message, Role,
    StreamSink, ToolDefinition,
};
use crate::config::{ImageInput, ProviderSpec};

/// Stands in for the images of a user (or system) message.
const IMAGE_PLACEHOLDER: &str = "(image omitted: this model can't view images)";

/// Stands in for the images of a tool result.
const TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: this model can't view images)";

/// Images a request left out because its model can't take them, reported on
/// the response so the turn can tell the user once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagesOmitted {
    /// The model the request went to.
    pub model: String,
    /// How many images were replaced with a placeholder.
    pub count: usize,
    /// The model was found to be text-only when its provider turned an
    /// earlier request down, rather than declared so in `providers.toml`.
    pub learned: bool,
}

/// Models found to be text-only this run, keyed by endpoint and model.
///
/// Process-wide because a model's input kinds don't depend on which agent or
/// role asked, and chains are rebuilt (per background session, on reload)
/// far more often than the answer changes. Not persisted: a restart sends
/// images again, which the provider's next rejection corrects.
#[derive(Default)]
pub(crate) struct LearnedTextOnly(Mutex<HashSet<String>>);

impl LearnedTextOnly {
    /// The registry every provider built from config shares.
    pub(crate) fn shared() -> Arc<Self> {
        static SHARED: LazyLock<Arc<LearnedTextOnly>> =
            LazyLock::new(|| Arc::new(LearnedTextOnly::default()));
        Arc::clone(&SHARED)
    }

    /// The set, still usable after a panic elsewhere poisoned its lock: it
    /// holds only names, which a panic can't leave half-written.
    fn models(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn contains(&self, key: &str) -> bool {
        self.models().contains(key)
    }

    /// Record `key` as text-only. Returns whether it is newly recorded.
    fn insert(&self, key: &str) -> bool {
        self.models().insert(key.to_string())
    }
}

/// Wraps one provider and keeps images away from its model when the model
/// can't take them: up front when `providers.toml` declares it text-only or
/// an earlier request found it so, and by retrying once without them when
/// the provider turns a request down for carrying images.
pub(crate) struct ImageInputGuard {
    inner: Box<dyn InferenceProvider>,
    declared: ImageInput,
    /// Identifies the model in `learned`: its endpoint and name.
    learned_key: String,
    learned: Arc<LearnedTextOnly>,
}

impl ImageInputGuard {
    /// Guard `inner`, built from `spec`, recording what it learns in `learned`.
    pub(crate) fn new(
        inner: Box<dyn InferenceProvider>,
        spec: &ProviderSpec,
        learned: Arc<LearnedTextOnly>,
    ) -> Self {
        Self {
            inner,
            declared: spec.image_input,
            learned_key: format!("{}\n{}", spec.provider_url, spec.model.model),
            learned,
        }
    }

    /// Whether this request should go without images from the start, and
    /// whether that is because a rejection taught it.
    fn strip_up_front(&self) -> Option<bool> {
        match self.declared {
            ImageInput::TextOnly => Some(false),
            ImageInput::Accepted => None,
            ImageInput::Undeclared => self.learned.contains(&self.learned_key).then_some(true),
        }
    }

    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&TrackedSink<'_>>,
    ) -> Result<InferenceResponse, InferenceError> {
        match sink {
            Some(sink) => {
                self.inner
                    .complete_streaming(messages, tools, options, sink)
                    .await
            }
            None => self.inner.complete(messages, tools, options).await,
        }
    }

    /// Send `stripped` and report the images it left out on the response.
    async fn send_without_images(
        &self,
        stripped: &StrippedRequest,
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&TrackedSink<'_>>,
        learned: bool,
    ) -> Result<InferenceResponse, InferenceError> {
        let mut response = self.send(&stripped.messages, tools, options, sink).await?;
        response.images_omitted = Some(ImagesOmitted {
            model: self.inner.model_name().to_string(),
            count: stripped.omitted,
            learned,
        });
        Ok(response)
    }

    async fn run(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&TrackedSink<'_>>,
    ) -> Result<InferenceResponse, InferenceError> {
        if let Some(learned) = self.strip_up_front() {
            return match strip_images(messages) {
                Some(stripped) => {
                    self.send_without_images(&stripped, tools, options, sink, learned)
                        .await
                }
                None => self.send(messages, tools, options, sink).await,
            };
        }

        let err = match self.send(messages, tools, options, sink).await {
            Ok(response) => return Ok(response),
            Err(err) => err,
        };
        if self.declared != ImageInput::Undeclared || !err.rejects_image_input() {
            return Err(err);
        }
        let Some(stripped) = strip_images(messages) else {
            return Err(err);
        };
        if self.learned.insert(&self.learned_key) {
            tracing::warn!(
                model = self.inner.model_name(),
                error = %err,
                "provider turned down a request with images; sending this model no images for the rest of the run"
            );
        }
        if let Some(sink) = sink {
            sink.restart_if_needed();
        }
        self.send_without_images(&stripped, tools, options, sink, true)
            .await
    }
}

#[async_trait]
impl InferenceProvider for ImageInputGuard {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        self.run(messages, tools, options, None).await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: &dyn StreamSink,
    ) -> Result<InferenceResponse, InferenceError> {
        let tracked = TrackedSink::new(sink);
        self.run(messages, tools, options, Some(&tracked)).await
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

/// A request rewritten without its images.
struct StrippedRequest {
    messages: Vec<Message>,
    /// How many images were left out.
    omitted: usize,
}

/// `messages` with every image replaced by a placeholder in its message's
/// text, one placeholder per message however many images it held. `None`
/// when there are no images to leave out.
fn strip_images(messages: &[Message]) -> Option<StrippedRequest> {
    let omitted: usize = messages.iter().map(|message| message.images.len()).sum();
    if omitted == 0 {
        return None;
    }
    let messages = messages
        .iter()
        .map(|message| {
            if message.images.is_empty() {
                return message.clone();
            }
            let placeholder = if message.role == Role::Tool {
                TOOL_IMAGE_PLACEHOLDER
            } else {
                IMAGE_PLACEHOLDER
            };
            let mut stripped = message.clone();
            stripped.images.clear();
            if stripped.content.is_empty() {
                stripped.content = placeholder.to_string();
            } else {
                stripped.content.push('\n');
                stripped.content.push_str(placeholder);
            }
            stripped
        })
        .collect();
    Some(StrippedRequest { messages, omitted })
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index into recorded requests of known shape for clarity"
)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::config::{ModelSpec, ProviderKind};
    use crate::inference::ImageData;

    /// Records each request it gets and answers from a script, then with a
    /// plain reply once the script runs out.
    struct ScriptedProvider {
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
        script: Mutex<VecDeque<Result<InferenceResponse, InferenceError>>>,
    }

    impl ScriptedProvider {
        fn new(
            script: Vec<Result<InferenceResponse, InferenceError>>,
        ) -> (Self, Arc<Mutex<Vec<Vec<Message>>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let provider = Self {
                requests: Arc::clone(&requests),
                script: Mutex::new(script.into()),
            };
            (provider, requests)
        }

        /// Turns down every request that carries an image, the way Z.ai did
        /// in issue #463, and answers the rest.
        fn rejecting_images() -> (Self, Arc<Mutex<Vec<Vec<Message>>>>) {
            Self::new(Vec::new())
        }
    }

    #[async_trait]
    impl InferenceProvider for ScriptedProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.requests.lock().unwrap().push(messages.to_vec());
            if let Some(next) = self.script.lock().unwrap().pop_front() {
                return next;
            }
            if messages.iter().any(|message| !message.images.is_empty()) {
                return Err(zai_rejection());
            }
            Ok(InferenceResponse::new("ok".to_string(), Vec::new()))
        }

        fn model_name(&self) -> &'static str {
            "glm-5.3"
        }
    }

    fn zai_rejection() -> InferenceError {
        InferenceError::Api(
            r#"400 Bad Request: {"error":{"code":"1214","message":"messages.content.type is invalid"}}"#
                .to_string(),
        )
    }

    fn image() -> ImageData {
        ImageData {
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        }
    }

    fn spec(model: &str, image_input: ImageInput) -> ProviderSpec {
        ProviderSpec {
            name: "zai".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: model.to_string(),
            },
            provider_url: "https://api.z.ai/api/paas/v4".to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
            image_input,
        }
    }

    fn guard(provider: ScriptedProvider, image_input: ImageInput) -> ImageInputGuard {
        ImageInputGuard::new(
            Box::new(provider),
            &spec("glm-5.3", image_input),
            Arc::new(LearnedTextOnly::default()),
        )
    }

    /// A conversation with a user image and a two-image tool result.
    fn conversation_with_images() -> Vec<Message> {
        vec![
            Message::system("be helpful"),
            Message::user_with_images("what is this?", vec![image()]),
            Message::assistant("", None),
            Message::tool_with_images(
                "[Image: selfie.png, 1.0 KB]",
                "call-1",
                vec![image(), image()],
            ),
        ]
    }

    async fn ask(
        guard: &ImageInputGuard,
        messages: &[Message],
    ) -> Result<InferenceResponse, InferenceError> {
        guard
            .complete(messages, &[], &CompletionOptions::default())
            .await
    }

    #[tokio::test]
    async fn a_declared_text_only_model_gets_placeholders_and_history_keeps_its_images() {
        let (provider, requests) = ScriptedProvider::new(Vec::new());
        let guard = guard(provider, ImageInput::TextOnly);
        let messages = conversation_with_images();

        let response = ask(&guard, &messages).await.unwrap();

        let sent = requests.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "one request, no retry");
        let request = &sent[0];
        assert!(
            request.iter().all(|message| message.images.is_empty()),
            "no image reaches the provider"
        );
        assert_eq!(
            request[1].content,
            "what is this?\n(image omitted: this model can't view images)"
        );
        assert_eq!(
            request[3].content,
            "[Image: selfie.png, 1.0 KB]\n(tool image omitted: this model can't view images)",
            "a tool result's two images collapse into one placeholder"
        );
        assert_eq!(
            messages[1].images.len(),
            1,
            "the conversation keeps its images"
        );
        assert_eq!(
            messages[3].images.len(),
            2,
            "the conversation keeps its images"
        );
        assert_eq!(
            response.images_omitted,
            Some(ImagesOmitted {
                model: "glm-5.3".to_string(),
                count: 3,
                learned: false,
            })
        );
    }

    #[tokio::test]
    async fn an_image_with_no_text_becomes_just_the_placeholder() {
        let (provider, requests) = ScriptedProvider::new(Vec::new());
        let guard = guard(provider, ImageInput::TextOnly);

        ask(
            &guard,
            &[Message::user_with_images(
                "",
                vec![image(), image(), image()],
            )],
        )
        .await
        .unwrap();

        let sent = requests.lock().unwrap().clone();
        assert_eq!(sent[0][0].content, IMAGE_PLACEHOLDER);
    }

    #[tokio::test]
    async fn a_vision_or_undeclared_model_gets_its_images_unchanged() {
        for image_input in [ImageInput::Accepted, ImageInput::Undeclared] {
            let (provider, requests) =
                ScriptedProvider::new(vec![Ok(InferenceResponse::new("ok".into(), Vec::new()))]);
            let guard = guard(provider, image_input);
            let messages = conversation_with_images();

            let response = ask(&guard, &messages).await.unwrap();

            let sent = requests.lock().unwrap().clone();
            assert_eq!(sent.len(), 1, "{image_input:?}: one request");
            assert_eq!(
                sent[0][1].images.len(),
                1,
                "{image_input:?}: user image sent"
            );
            assert_eq!(
                sent[0][3].images.len(),
                2,
                "{image_input:?}: tool images sent"
            );
            assert_eq!(
                sent[0][1].content, "what is this?",
                "{image_input:?}: no placeholder"
            );
            assert_eq!(
                response.images_omitted, None,
                "{image_input:?}: nothing omitted"
            );
        }
    }

    #[tokio::test]
    async fn a_text_only_request_without_images_reports_nothing() {
        let (provider, requests) = ScriptedProvider::new(Vec::new());
        let guard = guard(provider, ImageInput::TextOnly);

        let response = ask(&guard, &[Message::user("hi")]).await.unwrap();

        assert_eq!(requests.lock().unwrap()[0][0].content, "hi");
        assert_eq!(response.images_omitted, None);
    }

    #[tokio::test]
    async fn an_image_rejection_is_retried_once_without_images_and_remembered() {
        let (provider, requests) = ScriptedProvider::rejecting_images();
        let guard = guard(provider, ImageInput::Undeclared);
        let messages = conversation_with_images();

        let response = ask(&guard, &messages).await.unwrap();

        {
            let sent = requests.lock().unwrap();
            assert_eq!(sent.len(), 2, "the rejected request and exactly one retry");
            assert_eq!(
                sent[0][1].images.len(),
                1,
                "the first try carried the image"
            );
            assert!(
                sent[1].iter().all(|message| message.images.is_empty()),
                "the retry carries no images"
            );
        }
        assert_eq!(
            response.images_omitted,
            Some(ImagesOmitted {
                model: "glm-5.3".to_string(),
                count: 3,
                learned: true,
            })
        );

        let next = ask(&guard, &messages).await.unwrap();
        let sent = requests.lock().unwrap();
        assert_eq!(
            sent.len(),
            3,
            "the next request goes once, already without images"
        );
        assert!(sent[2].iter().all(|message| message.images.is_empty()));
        assert_eq!(
            next.images_omitted.map(|omitted| omitted.learned),
            Some(true)
        );
    }

    #[tokio::test]
    async fn what_one_guard_learns_reaches_a_guard_rebuilt_for_the_same_model() {
        let learned = Arc::new(LearnedTextOnly::default());
        let (first, _) = ScriptedProvider::rejecting_images();
        let first = ImageInputGuard::new(
            Box::new(first),
            &spec("glm-5.3", ImageInput::Undeclared),
            Arc::clone(&learned),
        );
        ask(&first, &conversation_with_images()).await.unwrap();

        let (second, second_requests) = ScriptedProvider::rejecting_images();
        let second = ImageInputGuard::new(
            Box::new(second),
            &spec("glm-5.3", ImageInput::Undeclared),
            Arc::clone(&learned),
        );
        ask(&second, &conversation_with_images()).await.unwrap();
        assert_eq!(
            second_requests.lock().unwrap().len(),
            1,
            "no rejected first try"
        );

        let (other, other_requests) = ScriptedProvider::new(Vec::new());
        let other = ImageInputGuard::new(
            Box::new(other),
            &spec("glm-5v", ImageInput::Undeclared),
            learned,
        );
        ask(&other, &conversation_with_images()).await.unwrap();
        assert_eq!(
            other_requests.lock().unwrap()[0][1].images.len(),
            1,
            "another model on the same endpoint still gets images"
        );
    }

    #[tokio::test]
    async fn a_rejection_that_is_not_about_images_is_not_retried() {
        let bad_request = || {
            Err(InferenceError::Api(
                r#"400 Bad Request: {"error":{"message":"Invalid value for 'temperature'"}}"#
                    .into(),
            ))
        };
        let (provider, requests) = ScriptedProvider::new(vec![bad_request(), bad_request()]);
        let guard = guard(provider, ImageInput::Undeclared);
        let messages = conversation_with_images();

        let err = ask(&guard, &messages).await.unwrap_err();

        assert!(
            err.to_string().contains("temperature"),
            "the error comes back: {err}"
        );
        assert_eq!(requests.lock().unwrap().len(), 1, "not retried");
        ask(&guard, &messages).await.unwrap_err();
        assert_eq!(
            requests.lock().unwrap()[1][1].images.len(),
            1,
            "the model is not marked text-only"
        );
    }

    #[tokio::test]
    async fn a_model_declared_to_take_images_is_not_second_guessed() {
        let (provider, requests) = ScriptedProvider::new(vec![Err(zai_rejection())]);
        let guard = guard(provider, ImageInput::Accepted);

        let err = ask(&guard, &conversation_with_images()).await.unwrap_err();

        assert!(err.rejects_image_input());
        assert_eq!(requests.lock().unwrap().len(), 1, "no stripped retry");
    }

    #[tokio::test]
    async fn an_image_rejection_without_images_to_strip_is_returned() {
        let (provider, requests) = ScriptedProvider::new(vec![Err(zai_rejection())]);
        let guard = guard(provider, ImageInput::Undeclared);

        let err = ask(&guard, &[Message::user("hi")]).await.unwrap_err();

        assert!(err.rejects_image_input());
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "nothing to retry without"
        );
    }
}

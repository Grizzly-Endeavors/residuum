//! Keeping images away from models that can't take them.
//!
//! Every provider a chain builds is wrapped in an [`ImageInputGuard`], so the
//! rewrite happens once, before any provider-specific conversion, for every
//! provider and every failover hop alike. The conversation itself is never
//! changed: only the outgoing request is, so switching back to a model that
//! takes images sends them again. Which hop of a failover chain gets a
//! request that carries images is decided in `failover`, which prefers a hop
//! that takes them.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;

use super::attempt::ListsAttempts;
use super::stream::TrackedSink;
use super::{
    CompletionOptions, ImageData, InferenceError, InferenceProvider, InferenceResponse, Message,
    Role, StreamSink, ToolDefinition,
};
use residuum_config::{ImageInput, ProviderSpec};

/// Maximum width or height in pixels that model APIs (such as Anthropic's)
/// accept for an inline image.
const MAX_IMAGE_DIMENSION: u32 = 8000;

/// Stands in for the images of a user (or system) message.
const IMAGE_PLACEHOLDER: &str = "(image omitted: this model can't view images)";

/// Stands in for the images of a tool result.
const TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: this model can't view images)";

/// Stands in for an oversized image in stored history whose bytes could not
/// be decoded to downscale.
const CORRUPT_OVERSIZED_IMAGE_PLACEHOLDER: &str =
    "(image omitted: image exceeds 8000 pixels and could not be decoded to downscale)";

/// Stands in for an oversized tool-result image in stored history whose
/// bytes could not be decoded to downscale.
const CORRUPT_OVERSIZED_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: image exceeds 8000 pixels and could not be decoded to downscale)";

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

    /// Whether the model is known to be text-only, and whether a rejection
    /// taught it. `Some` means requests to it go without images from the start.
    fn strip_up_front(&self) -> Option<bool> {
        match self.declared {
            ImageInput::TextOnly => Some(false),
            ImageInput::Accepted => None,
            ImageInput::Undeclared => self.learned.contains(&self.learned_key).then_some(true),
        }
    }

    /// The request as it goes out from the start, when the model is known to
    /// be text-only and the request carries images, with whether a rejection
    /// taught it. `None` means the request goes out as it is.
    fn stripped_up_front(&self, messages: &[Message]) -> Option<(StrippedRequest, bool)> {
        let learned = self.strip_up_front()?;
        strip_images(messages).map(|stripped| (stripped, learned))
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

    /// Send the request, keeping images from a model that can't take them.
    /// `image_fallback` says a later hop of the chain takes images, so an
    /// image rejection goes back to the chain, with the model remembered as
    /// text-only, instead of being retried here.
    async fn run(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&TrackedSink<'_>>,
        image_fallback: bool,
    ) -> Result<InferenceResponse, InferenceError> {
        if let Some((stripped, learned)) = self.stripped_up_front(messages) {
            return self
                .send_without_images(&stripped, tools, options, sink, learned)
                .await;
        }

        let normalized = normalize_oversized_images(messages);
        let outbound = normalized.as_deref().unwrap_or(messages);

        let err = match self.send(outbound, tools, options, sink).await {
            Ok(response) => return Ok(response),
            Err(err) => err,
        };
        if self.declared != ImageInput::Undeclared || !err.rejects_image_input() {
            return Err(err);
        }
        let Some(stripped) = strip_images(messages) else {
            return Err(err);
        };
        if image_fallback {
            // A later hop takes images, so a wrong verdict here only sends
            // image requests there: remember it now, and the chain passes
            // this hop over for every later request with images instead of
            // paying a refused request each time.
            self.learn(&err);
            return Err(err);
        }
        if let Some(sink) = sink {
            sink.restart_if_needed();
        }
        // Only a retry that succeeds without the images shows they were the
        // problem; a retry refused again leaves the model's images alone.
        let response = self
            .send_without_images(&stripped, tools, options, sink, true)
            .await
            .after_attempts(err.attempts().to_vec())?;
        self.learn(&err);
        Ok(response)
    }

    /// Remember the model as text-only, logging it the first time.
    fn learn(&self, rejection: &InferenceError) {
        if self.learned.insert(&self.learned_key) {
            tracing::warn!(
                model = self.inner.model_name(),
                error = %rejection,
                "provider turned down a request for its images; \
                 sending this model no images for the rest of the run"
            );
        }
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
        self.run(messages, tools, options, None, false).await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: &dyn StreamSink,
    ) -> Result<InferenceResponse, InferenceError> {
        let tracked = TrackedSink::new(sink);
        self.run(messages, tools, options, Some(&tracked), false)
            .await
    }

    async fn complete_as_hop(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&dyn StreamSink>,
        image_fallback: bool,
    ) -> Result<InferenceResponse, InferenceError> {
        let tracked = sink.map(TrackedSink::new);
        self.run(messages, tools, options, tracked.as_ref(), image_fallback)
            .await
    }

    fn takes_images(&self) -> bool {
        self.strip_up_front().is_none()
    }

    /// The inner provider's, over the request as this guard sends it: with
    /// the images left out when they are stripped up front, and any oversized
    /// images in history downscaled tomatch the outbound request.
    fn replayed_reasoning(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Vec<String> {
        if let Some((stripped, _)) = self.stripped_up_front(messages) {
            self.inner
                .replayed_reasoning(&stripped.messages, tools, options)
        } else {
            let normalized = normalize_oversized_images(messages);
            let outbound = normalized.as_deref().unwrap_or(messages);
            self.inner.replayed_reasoning(outbound, tools, options)
        }
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

/// Original and downscaled `(width, height)` pixel dimensions.
pub type DownscaledDimensions = ((u32, u32), (u32, u32));

/// Encode `bytes` as an inline [`ImageData`], downscaling the image in memory
/// when its width or height exceeds `8000` pixels.
///
/// Images within `8000×8000` pixels (and inputs whose dimensions cannot be
/// read from a supported image header) pass through untouched without
/// decoding or re-encoding. When the image is downscaled, returns the
/// `((original_width, original_height), (resized_width, resized_height))`
/// alongside the [`ImageData`].
///
/// # Errors
/// Returns an error if the image header declares a dimension over `8000`
/// pixels and the image bytes cannot be decoded or re-encoded.
pub fn prepare_inline_image(
    bytes: &[u8],
    fallback_media_type: &str,
) -> Result<(ImageData, Option<DownscaledDimensions>), String> {
    use base64::Engine as _;

    let media_type = super::sniff_image_media_type(bytes).unwrap_or(fallback_media_type);
    if let Some((format, width, height)) = inspect_image_header(bytes)
        && (width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION)
    {
        let downscaled = downscale_image_bytes(bytes, format, width, height)?;
        let data = base64::engine::general_purpose::STANDARD.encode(&downscaled.bytes);
        return Ok((
            ImageData {
                media_type: downscaled.media_type.to_string(),
                data,
            },
            Some(((width, height), downscaled.dims)),
        ));
    }

    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok((
        ImageData {
            media_type: media_type.to_string(),
            data,
        },
        None,
    ))
}

/// Read the image format and `(width, height)` from `bytes` without decoding
/// the full pixel payload.
fn inspect_image_header(bytes: &[u8]) -> Option<(image::ImageFormat, u32, u32)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    if format == image::ImageFormat::Gif && !has_valid_gif_header(bytes) {
        return None;
    }
    let (width, height) = reader.into_dimensions().ok()?;
    Some((format, width, height))
}

/// Whether `bytes` has a valid GIF header and Logical Screen Descriptor
/// followed by a GIF block introducer (`0x21` extension, `0x2C` image
/// descriptor, or `0x3B` trailer).
///
/// Unlike PNG, JPEG, and WebP header readers, the GIF header reader only reads
/// bytes `6..10` as little-endian `(u16, u16)` without checking any framing
/// byte after the descriptor, which would treat arbitrary ASCII after
/// `"GIF89a"` (e.g. `"GIF89a fake gif"`) as a `26144×27489` image.
fn has_valid_gif_header(bytes: &[u8]) -> bool {
    let Some(&packed) = bytes.get(10) else {
        return false;
    };
    let gct_bytes = if (packed & 0x80) != 0 {
        3_usize << ((packed & 0x07) + 1)
    } else {
        0
    };
    matches!(
        bytes.get(13_usize.saturating_add(gct_bytes)),
        Some(0x21 | 0x2C | 0x3B)
    )
}

struct DownscaledImage {
    bytes: Vec<u8>,
    media_type: &'static str,
    dims: (u32, u32),
}

/// Decode `bytes`, resize so the longest side is at most [`MAX_IMAGE_DIMENSION`]
/// preserving aspect ratio, and re-encode in `format`.
fn downscale_image_bytes(
    bytes: &[u8],
    format: image::ImageFormat,
    width: u32,
    height: u32,
) -> Result<DownscaledImage, String> {
    let decoded = image::load_from_memory_with_format(bytes, format).map_err(|e| {
        format!(
            "image is {width}\u{00d7}{height} px (exceeds {MAX_IMAGE_DIMENSION} px inline limit) \
             and could not be decoded to downscale: {e}"
        )
    })?;
    let resized = decoded.resize(
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_DIMENSION,
        image::imageops::FilterType::Triangle,
    );
    let dims = (resized.width(), resized.height());
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    let (media_type, encode_result) = if format == image::ImageFormat::Jpeg {
        (
            "image/jpeg",
            resized
                .into_rgb8()
                .write_to(&mut cursor, image::ImageFormat::Jpeg),
        )
    } else if format == image::ImageFormat::WebP {
        (
            "image/webp",
            resized
                .into_rgba8()
                .write_to(&mut cursor, image::ImageFormat::WebP),
        )
    } else if format == image::ImageFormat::Gif {
        (
            "image/gif",
            resized
                .into_rgba8()
                .write_to(&mut cursor, image::ImageFormat::Gif),
        )
    } else {
        (
            "image/png",
            resized.write_to(&mut cursor, image::ImageFormat::Png),
        )
    };
    encode_result.map_err(|e| {
        format!(
            "failed to encode downscaled {media_type} image \
             ({width}\u{00d7}{height} -> {}\u{00d7}{}): {e}",
            dims.0, dims.1
        )
    })?;
    Ok(DownscaledImage {
        bytes: out,
        media_type,
        dims,
    })
}

/// Check whether a stored [`ImageData`] in conversation history exceeds
/// [`MAX_IMAGE_DIMENSION`] in width or height, returning its decoded bytes,
/// format, and `(width, height)` when it does.
fn stored_image_exceeds_limit(
    image: &ImageData,
) -> Option<(Vec<u8>, image::ImageFormat, u32, u32)> {
    use base64::Engine as _;

    // PNG, GIF, and WebP store their width and height in the first 30 bytes
    // (40 base64 characters). Checking a 64-byte (88-char) prefix avoids
    // decoding the full base64 payload when the image already fits.
    if (image.data.starts_with("iVBOR")
        || image.data.starts_with("R0lGO")
        || image.data.starts_with("UklGR"))
        && let Some(prefix) = image.data.get(..88)
        && let Ok(header_bytes) = base64::engine::general_purpose::STANDARD.decode(prefix)
        && let Some((_, width, height)) = inspect_image_header(&header_bytes)
        && width <= MAX_IMAGE_DIMENSION
        && height <= MAX_IMAGE_DIMENSION
    {
        return None;
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.data)
        .ok()?;
    let (format, width, height) = inspect_image_header(&bytes)?;
    (width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION)
        .then_some((bytes, format, width, height))
}

/// Rewrite `messages` when any stored image exceeds [`MAX_IMAGE_DIMENSION`],
/// downscaling it for the outbound request (or replacing it with a placeholder
/// if its bytes are corrupt) so an already-stored oversized image in history
/// never bricks the conversation. Returns `None` when every image fits.
fn normalize_oversized_images(messages: &[Message]) -> Option<Vec<Message>> {
    use base64::Engine as _;

    let any_oversized = messages.iter().any(|msg| {
        msg.images
            .iter()
            .any(|img| stored_image_exceeds_limit(img).is_some())
    });
    if !any_oversized {
        return None;
    }

    let rewritten = messages
        .iter()
        .map(|message| {
            if message.images.is_empty() {
                return message.clone();
            }
            let mut updated = message.clone();
            let mut next_images = Vec::with_capacity(message.images.len());
            let mut omitted_corrupt = false;
            for img in &message.images {
                let Some((bytes, format, width, height)) = stored_image_exceeds_limit(img) else {
                    next_images.push(img.clone());
                    continue;
                };
                match downscale_image_bytes(&bytes, format, width, height) {
                    Ok(downscaled) => {
                        tracing::info!(
                            original_width = width,
                            original_height = height,
                            resized_width = downscaled.dims.0,
                            resized_height = downscaled.dims.1,
                            "downscaled oversized image from conversation history for outbound request"
                        );
                        next_images.push(ImageData {
                            media_type: downscaled.media_type.to_string(),
                            data: base64::engine::general_purpose::STANDARD
                                .encode(&downscaled.bytes),
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "omitting corrupt oversized image from conversation history for outbound request"
                        );
                        omitted_corrupt = true;
                    }
                }
            }
            updated.images = next_images;
            if omitted_corrupt {
                let placeholder = if updated.role == Role::Tool {
                    CORRUPT_OVERSIZED_TOOL_IMAGE_PLACEHOLDER
                } else {
                    CORRUPT_OVERSIZED_IMAGE_PLACEHOLDER
                };
                if updated.content.is_empty() {
                    updated.content = placeholder.to_string();
                } else {
                    updated.content.push('\n');
                    updated.content.push_str(placeholder);
                }
            }
            updated
        })
        .collect();
    Some(rewritten)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index into recorded requests of known shape for clarity"
)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::test_support::RecordingSink;
    use crate::{ImageData, StreamDelta};
    use residuum_config::{ModelSpec, ProviderKind};

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

    #[tokio::test]
    async fn a_retry_refused_again_teaches_nothing() {
        let (provider, requests) =
            ScriptedProvider::new(vec![Err(zai_rejection()), Err(zai_rejection())]);
        let guard = guard(provider, ImageInput::Undeclared);
        let messages = conversation_with_images();

        let err = ask(&guard, &messages).await.unwrap_err();

        assert!(err.rejects_image_input(), "the retry's error comes back");
        assert_eq!(requests.lock().unwrap().len(), 2, "one retry, no more");
        ask(&guard, &messages).await.unwrap();
        assert_eq!(
            requests.lock().unwrap()[2][1].images.len(),
            1,
            "the next request still carries its images"
        );
    }

    #[tokio::test]
    async fn a_hop_with_a_fallback_that_takes_images_returns_the_rejection() {
        let (provider, requests) = ScriptedProvider::rejecting_images();
        let guard = guard(provider, ImageInput::Undeclared);

        let err = guard
            .complete_as_hop(
                &conversation_with_images(),
                &[],
                &CompletionOptions::default(),
                None,
                true,
            )
            .await
            .unwrap_err();

        assert!(err.rejects_image_input());
        assert_eq!(requests.lock().unwrap().len(), 1, "no retry without images");
        assert!(
            !guard.takes_images(),
            "the rejection marks the model text-only, so the chain passes it over"
        );
    }

    /// Streams a piece of its answer before turning down a request with
    /// images, and answers one without them.
    struct StreamingRejecter {
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl InferenceProvider for StreamingRejecter {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Err(InferenceError::Api("this test only streams".to_string()))
        }

        async fn complete_streaming(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
            sink: &dyn StreamSink,
        ) -> Result<InferenceResponse, InferenceError> {
            self.requests.lock().unwrap().push(messages.to_vec());
            if messages.iter().any(|message| !message.images.is_empty()) {
                sink.push(StreamDelta::Text("I see a".to_string()));
                return Err(zai_rejection());
            }
            sink.push(StreamDelta::Text("no image here".to_string()));
            Ok(InferenceResponse::new(
                "no image here".to_string(),
                Vec::new(),
            ))
        }

        fn model_name(&self) -> &'static str {
            "glm-5.3"
        }
    }

    #[tokio::test]
    async fn a_streamed_rejection_is_voided_before_the_retry_streams() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let guard = ImageInputGuard::new(
            Box::new(StreamingRejecter {
                requests: Arc::clone(&requests),
            }),
            &spec("glm-5.3", ImageInput::Undeclared),
            Arc::new(LearnedTextOnly::default()),
        );
        let sink = RecordingSink::default();

        let response = guard
            .complete_streaming(
                &conversation_with_images(),
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await
            .unwrap();

        assert_eq!(
            sink.deltas(),
            vec![
                StreamDelta::Text("I see a".to_string()),
                StreamDelta::Restart,
                StreamDelta::Text("no image here".to_string()),
            ],
            "the rejected attempt's text is voided before the retry's"
        );
        assert_eq!(sink.text(), "no image here");
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(
            sent[1].iter().all(|message| message.images.is_empty()),
            "the retry carries no images"
        );
        assert_eq!(
            response.images_omitted.map(|omitted| omitted.learned),
            Some(true)
        );
    }

    fn encode_test_image(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([32, 128, 224]),
        ));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    fn decoded_dimensions(data: &str) -> (u32, u32) {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
    }

    #[test]
    fn prepare_inline_image_leaves_images_within_8000_pixels_untouched() {
        use base64::Engine as _;

        let raw = encode_test_image(120, 80, image::ImageFormat::Png);
        let (prepared, downscaled) = prepare_inline_image(&raw, "image/png").unwrap();

        assert_eq!(
            downscaled, None,
            "an image within 8000x8000 is not marked downscaled"
        );
        assert_eq!(prepared.media_type, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&prepared.data)
            .unwrap();
        assert_eq!(
            decoded, raw,
            "an image within 8000x8000 keeps its exact original bytes"
        );
    }

    #[test]
    fn prepare_inline_image_downscales_oversized_landscape_and_portrait_images() {
        for (format, expected_mime, orig_w, orig_h, expected_w, expected_h) in [
            (image::ImageFormat::Jpeg, "image/jpeg", 8448, 32, 8000, 30),
            (image::ImageFormat::Png, "image/png", 32, 8448, 30, 8000),
            (image::ImageFormat::WebP, "image/webp", 8200, 16, 8000, 16),
            (image::ImageFormat::Gif, "image/gif", 8200, 16, 8000, 16),
        ] {
            let raw = encode_test_image(orig_w, orig_h, format);
            let (prepared, downscaled) = prepare_inline_image(&raw, "application/octet-stream")
                .expect("oversized valid image should downscale");

            assert_eq!(
                prepared.media_type, expected_mime,
                "{expected_mime}: media type should match format"
            );
            assert_eq!(
                downscaled,
                Some(((orig_w, orig_h), (expected_w, expected_h))),
                "{expected_mime}: should report original and resized dimensions"
            );
            assert_eq!(
                decoded_dimensions(&prepared.data),
                (expected_w, expected_h),
                "{expected_mime}: re-encoded image dimensions should fit within 8000 px"
            );
        }
    }

    fn corrupt_oversized_png() -> Vec<u8> {
        // Keep the PNG signature, IHDR chunk, and IDAT chunk header intact so
        // `into_dimensions()` reads the 8448x32 header, then truncate mid-IDAT
        // so full pixel decoding fails.
        let mut bytes = encode_test_image(8448, 32, image::ImageFormat::Png);
        bytes.truncate(48);
        bytes
    }

    #[test]
    fn prepare_inline_image_returns_error_when_oversized_header_has_corrupt_body() {
        let corrupt_png = corrupt_oversized_png();

        let err = prepare_inline_image(&corrupt_png, "image/png")
            .expect_err("corrupt oversized PNG must not be passed through raw");
        assert!(
            err.contains("8448\u{00d7}32") && err.contains("8000"),
            "error should name the oversized dimensions and 8000 px limit: {err}"
        );
    }

    #[tokio::test]
    async fn an_oversized_image_in_stored_history_is_downscaled_on_the_outbound_request() {
        use base64::Engine as _;

        let raw = encode_test_image(8448, 32, image::ImageFormat::Jpeg);
        let oversized = ImageData {
            media_type: "image/jpeg".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&raw),
        };
        let messages = vec![Message::user_with_images(
            "check the trailcam",
            vec![oversized],
        )];
        let (provider, requests) =
            ScriptedProvider::new(vec![Ok(InferenceResponse::new("clear".into(), Vec::new()))]);
        let guard = guard(provider, ImageInput::Accepted);

        ask(&guard, &messages).await.unwrap();

        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 1, "one outbound request");
        assert_eq!(
            sent[0][0].images.len(),
            1,
            "the image is still sent to a vision-capable model"
        );
        assert_eq!(
            decoded_dimensions(&sent[0][0].images[0].data),
            (8000, 30),
            "the outbound image is downscaled to fit within 8000 px"
        );
        assert_eq!(
            decoded_dimensions(&messages[0].images[0].data),
            (8448, 32),
            "the caller's slice is not mutated in place"
        );
    }

    #[tokio::test]
    async fn a_corrupt_oversized_image_in_stored_history_is_replaced_with_a_placeholder() {
        use base64::Engine as _;

        let corrupt_png = corrupt_oversized_png();

        let corrupt = ImageData {
            media_type: "image/png".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&corrupt_png),
        };
        let messages = vec![Message::tool_with_images(
            "[Image: creek.png, 1.0 KB]",
            "call-1",
            vec![corrupt],
        )];
        let (provider, requests) =
            ScriptedProvider::new(vec![Ok(InferenceResponse::new("ok".into(), Vec::new()))]);
        let guard = guard(provider, ImageInput::Accepted);

        ask(&guard, &messages).await.unwrap();

        let sent = requests.lock().unwrap();
        assert!(
            sent[0][0].images.is_empty(),
            "corrupt oversized image is omitted instead of wedging the provider"
        );
        assert!(
            sent[0][0]
                .content
                .contains(CORRUPT_OVERSIZED_TOOL_IMAGE_PLACEHOLDER),
            "placeholder explains why the tool image was omitted: {}",
            sent[0][0].content
        );
    }
}

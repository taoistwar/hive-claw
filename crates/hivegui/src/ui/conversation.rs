use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures::StreamExt;
use gpui::{
    div, prelude::*, px, rgb, AsyncApp, Context, Entity, MouseButton, SharedString, Window,
};
use uuid::Uuid;

use crate::client::{self, streaming, OpenResponsesRequest};
use crate::model::conversation::{
    Attachment, AttachmentId, AttachmentPayload, Author, PendingTurnId, TurnContent, TurnError,
    TurnErrorKind, TurnStatus, MAX_ATTACHMENTS_PER_TURN, TOTAL_ATTACHMENTS_MAX_BYTES,
};
use crate::ui::app::{AppRoute, HiveGuiApp};
use crate::ui::input::TextInput;
use crate::ui::strings_zh;

/// Local view state for the conversation surface. Owns the editor's
/// in-flight text input and a transient error banner shown next to
/// the input surface (FR-007b oversize / count rejection).
pub struct ConversationView {
    editor_input: Entity<TextInput>,
    transient_error: Option<SharedString>,
}

impl ConversationView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        ConversationView {
            editor_input: cx
                .new(|cx| TextInput::new(cx).with_placeholder(strings_zh::SEND_PLACEHOLDER)),
            transient_error: None,
        }
    }
}

impl Render for ConversationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = {
            let app = cx.global::<HiveGuiApp>();
            app.conversation.read(cx).is_busy()
        };

        let pending = cx
            .global::<HiveGuiApp>()
            .pending_input
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default();

        self.transient_error = pending.transient_error.clone().map(SharedString::from);

        let mut turns_col = div()
            .id("turns")
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(16.0))
            .flex_grow();

        let snapshot: Vec<TurnSnapshot> = {
            let app = cx.global::<HiveGuiApp>();
            app.conversation
                .read(cx)
                .turns()
                .iter()
                .map(TurnSnapshot::from)
                .collect()
        };

        for turn in snapshot {
            let speaker = match turn.author {
                Author::User => strings_zh::SPEAKER_USER,
                Author::Assistant => strings_zh::SPEAKER_ASSISTANT,
            };
            let mut row = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_color(rgb(0x666666))
                        .text_size(px(12.0))
                        .child(speaker.to_string()),
                )
                .child(
                    div()
                        .text_color(rgb(0x111111))
                        .text_size(px(14.0))
                        .child(turn.text.clone()),
                );

            for (filename, size_bytes, mime) in &turn.attachments {
                let chip_label = format!(
                    "📎 {} ({}, {})",
                    filename,
                    crate::model::format_size(*size_bytes),
                    mime
                );
                row = row.child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(0xeef0f4))
                        .text_color(rgb(0x333333))
                        .text_size(px(12.0))
                        .child(chip_label),
                );
            }

            if let TurnStatus::Failed { retryable } = turn.status {
                let err_text = turn
                    .error_zh
                    .clone()
                    .unwrap_or_else(|| strings_zh::ERR_REPLY_FAILED.to_string());
                row = row.child(
                    div()
                        .text_color(rgb(0xb00020))
                        .text_size(px(12.0))
                        .child(err_text),
                );
                if retryable {
                    let turn_id = turn.id;
                    row = row.child(
                        div()
                            .id(("retry", turn_id.as_u128() as u64))
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(rgb(0xfff4f5))
                            .border_1()
                            .border_color(rgb(0xb00020))
                            .text_color(rgb(0xb00020))
                            .cursor_pointer()
                            .child(strings_zh::RETRY_BUTTON.to_string())
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                retry_turn(crate::model::conversation::TurnId(turn_id), cx);
                            }),
                    );
                }
            }
            turns_col = turns_col.child(row);
        }

        // Sync editor text to the global pending_input mirror
        {
            let content = self.editor_input.read(cx).content().to_string();
            if let Ok(mut p) = cx.global::<HiveGuiApp>().pending_input.lock() {
                if p.text != content {
                    p.text = content;
                }
            }
        }

        let editor_box = div()
            .id("editor")
            .px(px(12.0))
            .py(px(8.0))
            .min_h(px(56.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(0xd0d0d0))
            .bg(rgb(0xffffff))
            .child(self.editor_input.clone());

        let mut chip_row = div().flex().flex_row().gap(px(8.0));
        for (idx, a) in pending.attachments.iter().enumerate() {
            let label = format!(
                " {} ({}, {}) — {}",
                a.filename,
                crate::model::format_size(a.size_bytes),
                a.mime,
                strings_zh::ATTACHMENT_REMOVE
            );
            let idx_for_remove = idx;
            chip_row = chip_row.child(
                div()
                    .id(("chip", idx_for_remove as u64))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(0xeef0f4))
                    .text_color(rgb(0x333333))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .child(label)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        remove_pending_attachment_global(idx_for_remove, cx);
                    }),
            );
        }

        let attach_disabled = pending.attachments.len() >= MAX_ATTACHMENTS_PER_TURN || busy;
        let attach_button = {
            let attach_label = if attach_disabled {
                format!(
                    "{} ({}/{})",
                    strings_zh::ATTACH_BUTTON,
                    pending.attachments.len(),
                    MAX_ATTACHMENTS_PER_TURN
                )
            } else {
                strings_zh::ATTACH_BUTTON.to_string()
            };
            let mut b = div()
                .id("attach")
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(if attach_disabled {
                    rgb(0xdddddd)
                } else {
                    rgb(0xffffff)
                })
                .border_1()
                .border_color(rgb(0xd0d0d0))
                .text_color(rgb(0x111111))
                .child(attach_label);
            if !attach_disabled {
                b = b
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        spawn_attach_files(cx);
                    });
            }
            b
        };

        let send_disabled =
            pending.text.trim().is_empty() && pending.attachments.is_empty() || busy;
        let send_button = {
            let mut b = div()
                .id("send")
                .px(px(16.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(if send_disabled {
                    rgb(0xdddddd)
                } else {
                    rgb(0x111111)
                })
                .text_color(if send_disabled {
                    rgb(0x666666)
                } else {
                    rgb(0xffffff)
                })
                .child(strings_zh::SEND_BUTTON.to_string());
            if !send_disabled {
                b = b
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        spawn_send(cx);
                    });
            }
            b
        };

        let indicator = if busy {
            Some(
                div()
                    .text_color(rgb(0x666666))
                    .text_size(px(12.0))
                    .child(strings_zh::IN_PROGRESS),
            )
        } else {
            None
        };

        let transient = self.transient_error.clone().map(|msg| {
            div()
                .text_color(rgb(0xb00020))
                .text_size(px(12.0))
                .child(msg)
        });

        div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(12.0))
            .child(top_bar())
            .child(turns_col)
            .children(indicator)
            .child(chip_row)
            .children(transient)
            .child(editor_box)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(attach_button)
                    .child(send_button),
            )
    }
}

fn top_bar() -> impl IntoElement {
    div()
        .id("back")
        .px(px(12.0))
        .py(px(6.0))
        .text_color(rgb(0x444444))
        .cursor_pointer()
        .child(format!("← {}", strings_zh::HOME_TITLE))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.update_global::<HiveGuiApp, _>(|app, _| app.route = AppRoute::Home);
            cx.refresh_windows();
        })
}

struct TurnSnapshot {
    id: Uuid,
    author: Author,
    text: String,
    attachments: Vec<(String, u64, String)>,
    status: TurnStatus,
    error_zh: Option<String>,
}

impl From<&crate::model::conversation::ConversationTurn> for TurnSnapshot {
    fn from(t: &crate::model::conversation::ConversationTurn) -> Self {
        let (text, attachments) = match &t.content {
            TurnContent::UserMessage { text, attachments } => (
                text.clone(),
                attachments
                    .iter()
                    .map(|a| (a.filename.clone(), a.size_bytes, a.mime.clone()))
                    .collect(),
            ),
            TurnContent::AssistantText { buffer } => (buffer.clone(), Vec::new()),
        };
        TurnSnapshot {
            id: t.id.0,
            author: t.author,
            text,
            attachments,
            status: t.status.clone(),
            error_zh: t.error.as_ref().map(|e| e.message_zh.clone()),
        }
    }
}

// --- send / retry / attach pipelines ----------------------------------

fn retry_turn(turn_id: crate::model::conversation::TurnId, cx: &mut gpui::App) {
    let Some((pending, model, text, attachments, url, http)) =
        cx.update_global::<HiveGuiApp, _>(|app: &mut HiveGuiApp, cx| {
            let url = app.config.hiveclaw_url.clone();
            let http = app.http.clone();
            let (pending, text, attachments) = app.conversation.update(
                cx,
                |conv: &mut crate::model::conversation::Conversation, _| {
                    let new_pending = conv.retry(turn_id).ok()?;
                    let turn = conv
                        .turns()
                        .iter()
                        .find(|t| t.id == crate::model::conversation::TurnId(new_pending.0))?;
                    if let TurnContent::UserMessage { text, attachments } = &turn.content {
                        Some((new_pending, text.clone(), attachments.clone()))
                    } else {
                        None
                    }
                },
            )?;
            Some((
                pending,
                "openclaw:hiveclaw-placeholder-v1".to_string(),
                text,
                attachments,
                url,
                http,
            ))
        })
    else {
        return;
    };
    spawn_request(cx.to_async(), http, url, model, text, attachments, pending);
    cx.refresh_windows();
}

fn spawn_send(cx: &mut gpui::App) {
    // Read view state via the convention that ConversationView is the
    // active route's child. Because gpui's view-mutation lifetime is
    // tricky to thread through a global, we capture the state we need
    // by walking the app's `Conversation` model — the editor text +
    // attachments are *moved* into the model the moment we successfully
    // call `send_user_message`. Until that call, we read them from the
    // active ConversationView through `cx.update_global<HiveGuiApp>`
    // which routes back to the view via a callback hook.
    //
    // v1.1 simplification: the send pipeline reads `editor_text` and
    // `pending_attachments` directly off `HiveGuiApp` via a small
    // `pending_input` mirror that the view keeps in sync on every
    // render. See `HiveGuiApp::pending_input`.
    let Some((text, attachments)) =
        cx.update_global::<HiveGuiApp, _>(|app: &mut HiveGuiApp, _cx| {
            let p = app.pending_input.lock().ok()?.clone();
            Some((p.text, p.attachments))
        })
    else {
        return;
    };

    let url = cx.global::<HiveGuiApp>().config.hiveclaw_url.clone();
    let http = cx.global::<HiveGuiApp>().http.clone();
    let model = "openclaw:hiveclaw-placeholder-v1".to_string();

    let pending = cx.update_global::<HiveGuiApp, _>(|app: &mut HiveGuiApp, cx| {
        let r = app.conversation.update(
            cx,
            |conv: &mut crate::model::conversation::Conversation, _| {
                conv.send_user_message(text.clone(), attachments.clone())
            },
        );
        // Clear the pending-input mirror as soon as the model accepted.
        if r.is_ok() {
            if let Ok(mut p) = app.pending_input.lock() {
                *p = PendingInput::default();
            }
        }
        r.ok()
    });
    let Some(pending) = pending else {
        return;
    };

    spawn_request(cx.to_async(), http, url, model, text, attachments, pending);
    cx.refresh_windows();
}

fn spawn_request(
    async_cx: AsyncApp,
    http: Arc<reqwest::Client>,
    url: url::Url,
    model: String,
    text: String,
    attachments: Vec<Attachment>,
    pending: PendingTurnId,
) {
    async_cx
        .spawn(async move |cx: &mut AsyncApp| {
            let request_id = Uuid::new_v4();
            let req = OpenResponsesRequest::from_user_turn(model, &text, &attachments, true);
            let result = streaming::send(&http, &url, req, request_id).await;
            match result {
                Ok(mut stream) => {
                    while let Some(ev) = stream.next().await {
                        match ev {
                            Ok(streaming::StreamingEvent::Created { .. }) => {}
                            Ok(streaming::StreamingEvent::Delta { delta, .. }) => {
                                cx.update_global::<HiveGuiApp, _>(
                                    |app: &mut HiveGuiApp, cx| {
                                        app.conversation.update(cx, |conv: &mut crate::model::conversation::Conversation, _| {
                                            conv.append_assistant_chunk(pending, &delta);
                                        });
                                    },
                                );
                                cx.update(|cx| cx.refresh_windows());
                            }
                            Ok(streaming::StreamingEvent::Completed { full_text, .. }) => {
                                cx.update_global::<HiveGuiApp, _>(
                                    |app: &mut HiveGuiApp, cx| {
                                        app.conversation.update(cx, |conv: &mut crate::model::conversation::Conversation, _| {
                                            conv.record_assistant_reply(
                                                pending,
                                                crate::model::conversation::AssistantReply {
                                                    text: full_text,
                                                },
                                            );
                                        });
                                    },
                                );
                                cx.update(|cx| cx.refresh_windows());
                            }
                            Err(e) => {
                                record_failure(cx, pending, e).await;
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    // Network or upstream rejection before the stream opened.
                    record_failure(cx, pending, e).await;
                }
            }
        })
        .detach();
}

async fn record_failure(cx: &mut AsyncApp, pending: PendingTurnId, err: client::ClientError) {
    let (kind, message_zh) = classify_error(&err);
    cx.update_global::<HiveGuiApp, _>(|app: &mut HiveGuiApp, cx| {
        app.conversation.update(
            cx,
            |conv: &mut crate::model::conversation::Conversation, _| {
                conv.record_failure(pending, TurnError { kind, message_zh });
            },
        );
    });
    cx.update(|cx| cx.refresh_windows());
}

fn classify_error(err: &client::ClientError) -> (TurnErrorKind, String) {
    match err {
        client::ClientError::Unreachable(_) => (
            TurnErrorKind::Unreachable,
            strings_zh::ERR_HIVECLAW_UNREACHABLE.to_string(),
        ),
        client::ClientError::HttpStatus { status, .. } => (
            TurnErrorKind::ServerError,
            format!("{} ({})", strings_zh::ERR_REPLY_FAILED, status),
        ),
        client::ClientError::MalformedBody(_) | client::ClientError::StreamingProtocol(_) => (
            TurnErrorKind::TransportFailure,
            strings_zh::ERR_REPLY_FAILED.to_string(),
        ),
    }
}

fn spawn_attach_files(cx: &mut gpui::App) {
    let async_cx = cx.to_async();
    async_cx
        .spawn(async move |cx: &mut AsyncApp| {
            let paths = pick_files(cx).await;
            for path in paths {
                let _ = ingest_attachment(cx, path).await;
            }
            cx.update(|cx| cx.refresh_windows());
        })
        .detach();
}

async fn pick_files(cx: &AsyncApp) -> Vec<PathBuf> {
    // `App::prompt_for_paths` returns `oneshot::Receiver<Result<Option<Vec<PathBuf>>>>`.
    // We await the receiver, then unwrap the nested Result/Option to a flat
    // `Vec<PathBuf>` (empty on cancel / error / closed channel).
    let rx = cx.update(|cx| {
        cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Select files to attach".into()),
        })
    });
    match rx.await {
        Ok(Ok(Some(paths))) => paths,
        _ => Vec::new(),
    }
}

enum IngestError {
    ReadFailed,
    TooLarge,
    AlreadyAttached,
    TooMany,
    TotalTooLarge,
}

#[derive(Clone, Copy)]
enum IngestResult {
    Ok,
    LockFailed,
    AlreadyAttached,
    TooMany,
    TotalTooLarge,
}

async fn ingest_attachment(cx: &AsyncApp, path: PathBuf) -> Result<(), IngestError> {
    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "attachment".to_string());
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => {
            set_transient_error(
                cx,
                format!("{}：{}", strings_zh::ERR_FILE_READ_FAILED, filename),
            );
            return Err(IngestError::ReadFailed);
        }
    };
    if bytes.len() > crate::model::conversation::PER_FILE_MAX_BYTES_U64 as usize {
        set_transient_error(
            cx,
            format!("{}：{}", strings_zh::ERR_FILE_TOO_LARGE, filename),
        );
        return Err(IngestError::TooLarge);
    }
    let mime = mime_guess::from_path(&path)
        .first()
        .map(|m| m.essence_str().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let size_bytes = bytes.len() as u64;
    let base64_data_uri = format!("data:{};base64,{}", mime, B64.encode(&bytes));
    let attachment = Attachment {
        id: AttachmentId(Uuid::new_v4()),
        filename: filename.clone(),
        mime: mime.clone(),
        size_bytes,
        payload: AttachmentPayload::Inline { base64_data_uri },
    };

    let result = cx.update_global::<HiveGuiApp, _>(|app: &mut HiveGuiApp, _cx| {
        let mut p = match app.pending_input.lock() {
            Ok(p) => p,
            Err(_) => return IngestResult::LockFailed,
        };
        if p.attachments.iter().any(|a| a.filename == filename) {
            return IngestResult::AlreadyAttached;
        }
        if p.attachments.len() >= MAX_ATTACHMENTS_PER_TURN {
            return IngestResult::TooMany;
        }
        let new_total = p.total_bytes() + size_bytes;
        if new_total > TOTAL_ATTACHMENTS_MAX_BYTES {
            return IngestResult::TotalTooLarge;
        }
        p.attachments.push(attachment);
        IngestResult::Ok
    });

    match result {
        IngestResult::Ok => Ok(()),
        IngestResult::AlreadyAttached => {
            set_transient_error(
                cx,
                format!("{}：{}", strings_zh::ERR_FILE_ALREADY_ATTACHED, filename),
            );
            Err(IngestError::AlreadyAttached)
        }
        IngestResult::TooMany => {
            set_transient_error(cx, strings_zh::ERR_TOO_MANY_ATTACHMENTS.to_string());
            Err(IngestError::TooMany)
        }
        IngestResult::TotalTooLarge => {
            set_transient_error(cx, strings_zh::ERR_TOTAL_TOO_LARGE.to_string());
            Err(IngestError::TotalTooLarge)
        }
        IngestResult::LockFailed => {
            set_transient_error(cx, strings_zh::ERR_TOTAL_TOO_LARGE.to_string());
            Err(IngestError::TotalTooLarge)
        }
    }
}

fn set_transient_error(cx: &AsyncApp, msg: String) {
    cx.update_global::<HiveGuiApp, _>(|app: &mut HiveGuiApp, _cx| {
        if let Ok(mut p) = app.pending_input.lock() {
            p.transient_error = Some(msg);
        }
    });
    cx.update(|cx| cx.refresh_windows());
}

fn remove_pending_attachment_global(idx: usize, cx: &mut gpui::App) {
    cx.update_global::<HiveGuiApp, _>(|app, _cx| {
        if let Ok(mut p) = app.pending_input.lock() {
            if idx < p.attachments.len() {
                p.attachments.remove(idx);
            }
        }
    });
    cx.refresh_windows();
}

/// Mirror of the user's currently-pending input. The view writes into it
/// on every keystroke / attach, and `spawn_send` drains it. Mirroring
/// to `HiveGuiApp` (not the view) keeps the send dispatch decoupled
/// from view-mutation lifetimes.
#[derive(Default, Clone)]
pub struct PendingInput {
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub transient_error: Option<String>,
}

impl PendingInput {
    pub fn total_bytes(&self) -> u64 {
        self.attachments.iter().map(|a| a.size_bytes).sum()
    }
}

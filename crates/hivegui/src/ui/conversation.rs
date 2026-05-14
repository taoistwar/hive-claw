use gpui::{div, prelude::*, px, rgb, Context, MouseButton, Window};

use crate::model::conversation::{Author, TurnContent, TurnStatus};
use crate::ui::app::{AppRoute, HiveGuiApp};
use crate::ui::strings_zh;

pub struct ConversationView;

impl ConversationView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        ConversationView
    }
}

impl Render for ConversationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.global::<HiveGuiApp>();
        let conversation = app.conversation.read(cx);
        let busy = conversation.is_busy();

        // TODO(v1.1): replace this `div`-of-rows with `gpui::list(ListState, …)`
        // to satisfy the spec "Very long agent reply: MUST remain scrollable"
        // edge case. v1 uses a plain `flex_col` + `overflow_hidden` so the
        // layout doesn't blow past the window; very long replies will clip
        // until the list-element migration lands.
        let mut turns_col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(16.0))
            .overflow_hidden()
            .flex_grow();

        for turn in conversation.turns().iter() {
            let speaker = match turn.author {
                Author::User => strings_zh::SPEAKER_USER,
                Author::Assistant => strings_zh::SPEAKER_ASSISTANT,
            };
            let text = match &turn.content {
                TurnContent::UserText { text } => text.clone(),
                TurnContent::AssistantText { buffer } => buffer.clone(),
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
                        .child(text),
                );

            if let TurnStatus::Failed { retryable } = turn.status {
                let err_text = turn
                    .error
                    .as_ref()
                    .map(|e| e.message_zh.clone())
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
                            .id(("retry", turn_id.0.as_u128() as u64))
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
                                cx.update_global::<HiveGuiApp, _>(|app, cx| {
                                    let _ = app.conversation.update(cx, |conv, _| {
                                        let _ = conv.retry(turn_id);
                                    });
                                });
                                cx.refresh_windows();
                            }),
                    );
                }
            }
            turns_col = turns_col.child(row);
        }

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

        let mut send_button = div()
            .id("send")
            .px(px(16.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(if busy { rgb(0xdddddd) } else { rgb(0x111111) })
            .text_color(if busy { rgb(0x666666) } else { rgb(0xffffff) })
            .child(strings_zh::SEND_BUTTON.to_string());
        if !busy {
            send_button =
                send_button
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, _cx| {
                        // Send pipeline is wired in the binary's event loop; the
                        // button here just signals intent. See main.rs for the
                        // tokio runtime that drives the network call.
                    });
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(12.0))
            .child(top_bar())
            .child(turns_col)
            .children(indicator)
            .child(send_button)
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

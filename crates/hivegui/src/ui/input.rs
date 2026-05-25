use std::ops::Range;

use gpui::{
    actions, div, fill, hsla, prelude::*, px, rgba, App, Bounds, ClipboardItem, Context,
    CursorStyle, DefiniteLength, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, MouseButton, PaintQuad, Pixels, Point, ShapedLine, SharedString, UTF16Selection,
    UnderlineStyle, Window,
};

actions!(
    input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Submit,
    ]
);

type OnChangeCallback = Box<dyn Fn(&str)>;

/// A minimal single-line text input for gpui that can receive focus and keyboard input.
pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    /// All ranges are stored in **UTF-16 code unit offsets** as required by GPUI's
    /// `EntityInputHandler` trait. This avoids conversion errors with multi-byte
    /// characters like Chinese/Japanese/Korean text.
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    on_change: Option<OnChangeCallback>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        TextInput {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            on_change: None,
        }
    }

    pub fn with_placeholder(mut self, placeholder: &str) -> Self {
        self.placeholder = SharedString::from(placeholder);
        self
    }

    pub fn on_change<F: Fn(&str) + 'static>(mut self, cb: F) -> Self {
        self.on_change = Some(Box::new(cb));
        self
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_content(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = SharedString::from(text);
        self.selected_range = 0..0;
        cx.notify();
    }

    pub fn move_left(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    pub fn move_right(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    pub fn select_left(&mut self, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    pub fn select_right(&mut self, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content_utf16_len(), cx);
    }

    pub fn home(&mut self, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    pub fn end(&mut self, cx: &mut Context<Self>) {
        self.move_to(self.content_utf16_len(), cx);
    }

    pub fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            self.select_to(prev, cx);
        }
        self.replace_text_internal(None, "");
        cx.notify();
    }

    pub fn delete(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            self.select_to(next, cx);
        }
        self.replace_text_internal(None, "");
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(
        &mut self,
        _event: &gpui::MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_internal(None, &text.replace('\n', " "));
            cx.notify();
        }
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let range = self.utf16_range_to_string(&self.selected_range);
            cx.write_to_clipboard(ClipboardItem::new_string(range));
        }
    }

    pub fn cut(&mut self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let range = self.utf16_range_to_string(&self.selected_range);
            cx.write_to_clipboard(ClipboardItem::new_string(range));
            self.replace_text_internal(None, "");
            cx.notify();
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let bounds: &Bounds<Pixels> = match self.last_bounds.as_ref() {
            Some(b) => b,
            None => return 0,
        };
        let line: &ShapedLine = match self.last_layout.as_ref() {
            Some(l) => l,
            None => return 0,
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content_utf16_len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    /// Previous grapheme boundary in **UTF-16 offset** space.
    fn previous_boundary(&self, offset: usize) -> usize {
        let mut utf16_pos = 0;
        let mut prev = 0;
        for ch in self.content.chars() {
            let char_len = ch.len_utf16();
            if utf16_pos > 0 && utf16_pos >= offset {
                return prev;
            }
            prev = utf16_pos;
            utf16_pos += char_len;
        }
        prev
    }

    /// Next grapheme boundary in **UTF-16 offset** space.
    fn next_boundary(&self, offset: usize) -> usize {
        let total = self.content_utf16_len();
        let mut utf16_pos = 0;
        for ch in self.content.chars() {
            let char_len = ch.len_utf16();
            let next_pos = utf16_pos + char_len;
            if utf16_pos == offset || (utf16_pos < offset && next_pos >= offset) {
                return next_pos;
            }
            utf16_pos = next_pos;
        }
        total
    }

    /// Convert a UTF-8 byte offset to a UTF-16 code unit offset.
    fn utf8_to_utf16_offset(&self, utf8_offset: usize) -> usize {
        self.content[..utf8_offset.min(self.content.len())]
            .chars()
            .map(|c| c.len_utf16())
            .sum()
    }

    /// Convert a UTF-16 code unit offset to a UTF-8 byte offset.
    fn utf16_to_utf8_offset(&self, utf16_offset: usize) -> usize {
        let mut utf8_pos = 0;
        let mut utf16_pos = 0;
        for ch in self.content.chars() {
            if utf16_pos >= utf16_offset {
                break;
            }
            utf16_pos += ch.len_utf16();
            utf8_pos += ch.len_utf8();
        }
        utf8_pos
    }

    /// Get the total UTF-16 length of the content.
    fn content_utf16_len(&self) -> usize {
        self.content.chars().map(|c| c.len_utf16()).sum()
    }

    /// Extract a substring given a UTF-16 range.
    fn utf16_range_to_string(&self, range: &Range<usize>) -> String {
        let start = self.utf16_to_utf8_offset(range.start);
        let end = self.utf16_to_utf8_offset(range.end);
        self.content[start..end].to_string()
    }

    /// Replace text at the current selection or marked range.
    /// `new_text` is the string to insert.
    fn replace_text_internal(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
    ) {
        let range = range_utf16
            .clone()
            .unwrap_or_else(|| self.selected_range.clone());

        let start = self.utf16_to_utf8_offset(range.start);
        let end = self.utf16_to_utf8_offset(range.end);

        self.content =
            (self.content[..start].to_owned() + new_text + &self.content[end..]).into();

        let new_cursor = range.start + new_text.encode_utf16().count();
        self.selected_range = new_cursor..new_cursor;
        self.marked_range.take();

        if let Some(ref cb) = self.on_change {
            let text = self.content.to_string();
            cb(&text);
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let total = self.content_utf16_len();
        let start = range_utf16.start.min(total);
        let end = range_utf16.end.min(total);
        let actual = start..end;
        adjusted_range.replace(actual.clone());
        Some(self.utf16_range_to_string(&actual))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.selected_range.clone(),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        eprintln!("[TextInput] marked_text_range: {:?}", self.marked_range);
        self.marked_range.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        eprintln!("[TextInput] unmark_text");
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        eprintln!("[TextInput] replace_text_in_range: range={:?}, text={:?}, len={}", range_utf16, new_text, new_text.len());
        self.replace_text_internal(range_utf16, new_text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        eprintln!("[TextInput] replace_and_mark_text_in_range: range={:?}, text={:?}, sel={:?}", range_utf16, new_text, new_selected_range_utf16);
        let range = range_utf16
            .clone()
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let start = self.utf16_to_utf8_offset(range.start);
        let end = self.utf16_to_utf8_offset(range.end);

        self.content =
            (self.content[..start].to_owned() + new_text + &self.content[end..]).into();

        let new_text_utf16_len = new_text.encode_utf16().count();

        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text_utf16_len);
        } else {
            self.marked_range = None;
        }

        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| range.start + r.start..range.start + r.end)
            .unwrap_or_else(|| {
                let pos = range.start + new_text_utf16_len;
                pos..pos
            });

        if let Some(ref cb) = self.on_change {
            let text = self.content.to_string();
            cb(&text);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        // ShapedLine.x_for_index expects a byte index in the shaped text.
        // Since our shaped line is built from the content with char-count runs,
        // the index should match UTF-16 offset when using proper shaping.
        let start = self.utf16_to_utf8_offset(range_utf16.start);
        let end = self.utf16_to_utf8_offset(range_utf16.end);
        Some(Bounds::from_corners(
            Point::new(
                bounds.left() + last_layout.x_for_index(start),
                bounds.top(),
            ),
            Point::new(
                bounds.left() + last_layout.x_for_index(end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        // x_for_index returns x position for given byte index
        // We need to find the UTF-16 offset corresponding to that byte index
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        // Convert byte index to UTF-16
        Some(self.utf8_to_utf16_offset(utf8_index))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// --- Text rendering element ---

struct TextInputElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextInputElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let mut style = gpui::Style::default();
        style.size.width = gpui::Length::Definite(DefiniteLength::Fraction(1.0));
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor_utf16 = input.cursor_offset();
        let style = window.text_style();

        // Convert UTF-16 offset to byte index for ShapedLine methods.
        let utf16_to_utf8 = |s: &str, utf16_offset: usize| -> usize {
            let mut utf8_pos = 0;
            let mut utf16_pos = 0;
            for ch in s.chars() {
                if utf16_pos >= utf16_offset {
                    break;
                }
                utf16_pos += ch.len_utf16();
                utf8_pos += ch.len_utf8();
            }
            utf8_pos
        };

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0., 0.2))
        } else {
            (content, style.color)
        };

        // GPUI TextRun.len expects the count of Unicode scalar values (Rust chars).
        let text_char_count = display_text.chars().count();
        let run = gpui::TextRun {
            len: text_char_count,
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let marked_range = input.marked_range.clone();
        let runs = if let Some(marked_range) = marked_range.as_ref() {
            let start = utf16_to_utf8(&display_text, marked_range.start);
            let end = utf16_to_utf8(&display_text, marked_range.end);
            let before_chars = display_text[..start].chars().count();
            let marked_chars = display_text[start..end].chars().count();
            let after_chars = text_char_count - before_chars - marked_chars;

            let mut result_runs: Vec<gpui::TextRun> = Vec::new();
            if before_chars > 0 {
                result_runs.push(gpui::TextRun {
                    len: before_chars,
                    ..run.clone()
                });
            }
            result_runs.push(gpui::TextRun {
                len: marked_chars,
                underline: Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..run.clone()
            });
            if after_chars > 0 {
                result_runs.push(gpui::TextRun {
                    len: after_chars,
                    ..run
                });
            }
            result_runs
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text.clone(), font_size, &runs, None);

        // x_for_index expects byte index, convert from UTF-16
        let cursor_byte = utf16_to_utf8(&display_text, cursor_utf16);
        let cursor_pos = line.x_for_index(cursor_byte);
        let sel_start_byte = utf16_to_utf8(&display_text, selected_range.start);
        let sel_end_byte = utf16_to_utf8(&display_text, selected_range.end);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        Point::new(bounds.left() + cursor_pos, bounds.top()),
                        gpui::size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    gpui::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        Point::new(
                            bounds.left() + line.x_for_index(sel_start_byte),
                            bounds.top(),
                        ),
                        Point::new(
                            bounds.left() + line.x_for_index(sel_end_byte),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x3311ff30),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

// --- Render impl for TextInput ---

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Backspace, _window, cx| {
                this.backspace(cx);
            }))
            .on_action(cx.listener(|this, _: &Delete, _window, cx| {
                this.delete(cx);
            }))
            .on_action(cx.listener(|this, _: &Left, _window, cx| {
                this.move_left(cx);
            }))
            .on_action(cx.listener(|this, _: &Right, _window, cx| {
                this.move_right(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _window, cx| {
                this.select_left(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _window, cx| {
                this.select_right(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _window, cx| {
                this.select_all(cx);
            }))
            .on_action(cx.listener(|this, _: &Home, _window, cx| {
                this.home(cx);
            }))
            .on_action(cx.listener(|this, _: &End, _window, cx| {
                this.end(cx);
            }))
            .on_action(cx.listener(|_this, _: &ShowCharacterPalette, window, _cx| {
                window.show_character_palette();
            }))
            .on_action(cx.listener(|this, _: &Paste, _window, cx| {
                this.paste(cx);
            }))
            .on_action(cx.listener(|this, _: &Cut, _window, cx| {
                this.cut(cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, _window, cx| {
                this.copy(cx);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextInputElement { input: cx.entity() })
    }
}

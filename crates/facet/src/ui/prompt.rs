//! The text prompt dialog: a few labelled fields and an optional checkbox, for naming a new
//! branch, a tag, a remote, a stash — and for the credentials git asks for (G7). Like the
//! confirmation dialog it owns the keyboard: Enter submits, Escape cancels.

use eframe::egui::{
    self, text::LayoutJob, Align, CornerRadius, Frame, Id, Key, Layout, Margin, Modifiers, Order,
    Pos2, Rect, Sense, StrokeKind, TextEdit, TextFormat, Ui, UiBuilder, Vec2,
};

use super::widgets::{button_enabled, checkbox, ButtonKind};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::theme::{self, motion, radius, space, Palette};

const WIDTH: f32 = 460.0;
const FIELD_HEIGHT: f32 = 32.0;
const LABEL_HEIGHT: f32 = 20.0;

/// What the answer is for.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    /// Fields: name. Checkbox: switch to it.
    NewBranch { at: String },
    /// Fields: new name.
    RenameBranch { old: String },
    /// Fields: name, message (annotated when given).
    NewTag { at: String },
    /// Fields: name, URL.
    AddRemote,
    /// Fields: message. Checkbox: include untracked files (when stashing everything).
    Stash { paths: Vec<String>, untracked: bool },
    /// Fields: the answer git asked for.
    Askpass,
    /// Fields: file extensions, diff command, merge command.
    ExternalTool,
    /// Fields: branch, folder.
    AddWorktree,
    /// Fields: pattern. Checkbox: lockable.
    LfsTrack,
    /// Fields: name, email.
    CommitAuthor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptField {
    pub label: String,
    pub value: String,
    pub hint: String,
    /// Shown masked (passwords, passphrases).
    pub secret: bool,
    /// Submitting needs a non-blank value.
    pub required: bool,
}

impl PromptField {
    pub fn new(label: &str, hint: &str, required: bool) -> Self {
        Self {
            label: label.to_owned(),
            value: String::new(),
            hint: hint.to_owned(),
            secret: false,
            required,
        }
    }

    pub fn with_value(mut self, value: &str) -> Self {
        value.clone_into(&mut self.value);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub body: String,
    pub fields: Vec<PromptField>,
    pub check: Option<(String, bool)>,
    pub submit_label: String,
    /// Put the keyboard focus in the first field on the next frame.
    pub focus: bool,
}

impl Prompt {
    pub fn new(kind: PromptKind, title: impl Into<String>, submit_label: &str) -> Self {
        Self {
            kind,
            title: title.into(),
            body: String::new(),
            fields: Vec::new(),
            check: None,
            submit_label: submit_label.to_owned(),
            focus: true,
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn field(mut self, field: PromptField) -> Self {
        self.fields.push(field);
        self
    }

    pub fn check(mut self, label: &str, checked: bool) -> Self {
        self.check = Some((label.to_owned(), checked));
        self
    }

    pub fn can_submit(&self) -> bool {
        self.fields
            .iter()
            .all(|field| !field.required || !field.value.trim().is_empty())
    }

    /// The trimmed value of field `index` (secrets are passed on exactly as typed).
    pub fn value(&self, index: usize) -> String {
        self.fields.get(index).map_or_else(String::new, |field| {
            if field.secret {
                field.value.clone()
            } else {
                field.value.trim().to_owned()
            }
        })
    }

    pub fn checked(&self) -> bool {
        self.check.as_ref().is_some_and(|(_, checked)| *checked)
    }
}

impl FacetApp {
    pub(super) fn prompt_layer(&mut self, ui: &mut Ui, window: Rect) {
        let ctx = ui.ctx().clone();
        let visible = ctx.animate_bool_with_time_and_easing(
            Id::unique("prompt-visible"),
            self.prompt.is_some(),
            motion::NORMAL,
            motion::ease_out,
        );
        let palette = Palette::of(&ctx);
        let Some(prompt) = &mut self.prompt else {
            return;
        };

        let (mut submitted, mut cancelled) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::Enter),
                input.consume_key(Modifiers::NONE, Key::Escape),
            )
        });

        let width = WIDTH.min(window.width() - space::XL);
        let text_width = width - space::L * 2.0;
        let wrapped = |text: &str, font: egui::FontId, color: egui::Color32| {
            let mut job = LayoutJob::single_section(
                text.to_owned(),
                TextFormat {
                    font_id: font,
                    color,
                    ..Default::default()
                },
            );
            job.wrap.max_width = text_width;
            ctx.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let title = wrapped(&prompt.title, theme::strong(), palette.text);
        let body = (!prompt.body.is_empty())
            .then(|| wrapped(&prompt.body, theme::small(), palette.text_secondary));
        let fields_height = prompt.fields.len() as f32 * (LABEL_HEIGHT + FIELD_HEIGHT + space::S);
        let check_height = if prompt.check.is_some() {
            theme::ROW_HEIGHT
        } else {
            0.0
        };
        let height = space::L
            + title.size().y
            + body.as_ref().map_or(0.0, |body| space::XS + body.size().y)
            + space::M
            + fields_height
            + check_height
            + space::M
            + theme::ROW_HEIGHT
            + space::M;
        let panel = Rect::from_min_size(
            Pos2::new(
                window.center().x - width / 2.0,
                window.top() + (window.height() * 0.2).max(theme::TITLE_BAR_HEIGHT + space::M)
                    - (1.0 - visible) * space::S,
            ),
            Vec2::new(width, height),
        );

        egui::Area::new(Id::unique("prompt"))
            .order(Order::Foreground)
            .fixed_pos(window.min)
            .show(&ctx, |ui| {
                ui.set_opacity(visible);
                let scrim = ui.allocate_rect(window, Sense::click());
                ui.painter().rect_filled(window, 0.0, palette.scrim);
                if scrim.clicked() {
                    cancelled = true;
                }
                let corner = CornerRadius::same(radius::L);
                ui.painter()
                    .add(ui.visuals().window_shadow.as_shape(panel, corner));
                ui.painter().rect(
                    panel,
                    corner,
                    palette.raised,
                    palette.hairline_stroke(),
                    StrokeKind::Outside,
                );
                ui.interact(panel, Id::unique("prompt-panel"), Sense::click());

                let left = panel.left() + space::L;
                let mut y = panel.top() + space::L;
                let title_height = title.size().y;
                ui.painter().galley(Pos2::new(left, y), title, palette.text);
                y += title_height;
                if let Some(body) = body {
                    y += space::XS;
                    let body_height = body.size().y;
                    ui.painter()
                        .galley(Pos2::new(left, y), body, palette.text_secondary);
                    y += body_height;
                }
                y += space::M;

                let focus = std::mem::take(&mut prompt.focus);
                for (index, field) in prompt.fields.iter_mut().enumerate() {
                    ui.painter().text(
                        Pos2::new(left, y + LABEL_HEIGHT / 2.0),
                        egui::Align2::LEFT_CENTER,
                        &field.label,
                        theme::small_strong(),
                        palette.text_secondary,
                    );
                    y += LABEL_HEIGHT;
                    let rect = Rect::from_min_size(
                        Pos2::new(left, y),
                        Vec2::new(text_width, FIELD_HEIGHT),
                    );
                    let id = Id::unique(("prompt-field", index));
                    let has_focus = ctx.memory(|memory| memory.has_focus(id));
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(radius::M),
                        palette.surface,
                        if has_focus {
                            egui::Stroke::new(1.5, palette.accent)
                        } else {
                            palette.hairline_stroke()
                        },
                        StrokeKind::Inside,
                    );
                    let response = ui
                        .scope_builder(
                            UiBuilder::new()
                                .id_salt(("prompt-field-ui", index))
                                .max_rect(rect)
                                .layout(Layout::left_to_right(Align::Center)),
                            |ui| {
                                ui.add_sized(
                                    rect.size(),
                                    TextEdit::singleline(&mut field.value)
                                        .id(id)
                                        .password(field.secret)
                                        .hint_text(field.hint.as_str())
                                        .font(theme::body())
                                        .frame(
                                            Frame::NONE
                                                .inner_margin(Margin::symmetric(space::S as i8, 0)),
                                        )
                                        .align(egui::Align2::LEFT_CENTER),
                                )
                            },
                        )
                        .inner;
                    if focus && index == 0 {
                        response.request_focus();
                    }
                    y += FIELD_HEIGHT + space::S;
                }

                if let Some((label, checked)) = &mut prompt.check {
                    let rect = Rect::from_min_size(
                        Pos2::new(left, y),
                        Vec2::new(text_width, theme::ROW_HEIGHT),
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt("prompt-check")
                            .max_rect(rect)
                            .layout(Layout::left_to_right(Align::Center)),
                        |ui| {
                            if checkbox(ui, *checked, label, label, true).clicked() {
                                *checked = !*checked;
                            }
                        },
                    );
                }

                let row = Rect::from_min_max(
                    Pos2::new(left, panel.bottom() - space::M - theme::ROW_HEIGHT),
                    Pos2::new(panel.right() - space::M, panel.bottom() - space::M),
                );
                let can_submit = prompt.can_submit();
                ui.scope_builder(
                    UiBuilder::new()
                        .id_salt("prompt-buttons")
                        .max_rect(row)
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        if button_enabled(
                            ui,
                            ButtonKind::Primary,
                            None,
                            &prompt.submit_label,
                            &format!("{}  (Enter)", prompt.submit_label),
                            can_submit,
                        )
                        .clicked()
                        {
                            submitted = true;
                        }
                        if button_enabled(
                            ui,
                            ButtonKind::Secondary,
                            None,
                            "Cancel",
                            "Cancel  (Esc)",
                            true,
                        )
                        .clicked()
                        {
                            cancelled = true;
                        }
                    },
                );
            });

        if cancelled {
            self.pending.push(Action::CancelPrompt);
        } else if submitted && self.prompt.as_ref().is_some_and(Prompt::can_submit) {
            self.pending.push(Action::SubmitPrompt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_fields_gate_submission_and_values_are_trimmed() {
        let mut prompt = Prompt::new(PromptKind::AddRemote, "Add Remote", "Add")
            .field(PromptField::new("Name", "origin", true))
            .field(PromptField::new("URL", "https://…", true))
            .check("Fetch", true);
        assert!(!prompt.can_submit());
        prompt.fields[0].value = "  upstream ".into();
        assert!(!prompt.can_submit());
        prompt.fields[1].value = "https://example.com/a.git".into();
        assert!(prompt.can_submit());
        assert_eq!(prompt.value(0), "upstream");
        assert_eq!(prompt.value(9), "");
        assert!(prompt.checked());

        // Secrets keep their spaces.
        let mut secret = Prompt::new(PromptKind::Askpass, "Password", "OK")
            .field(PromptField::new("Password", "", false));
        secret.fields[0].secret = true;
        secret.fields[0].value = " pass word ".into();
        assert_eq!(secret.value(0), " pass word ");
        assert!(secret.can_submit());
    }
}

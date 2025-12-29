use crate::app::{NoteSummary, NotesApp};
use anyhow::{anyhow, Result};
use iced::widget::{
    button, column, container, row, scrollable, text, text_editor, text_input, Column,
};
use iced::{
    executor, Alignment, Application, Color, Command, Element, Length, Settings, Theme,
};

const ACCENT: Color = Color {
    r: 38.0 / 255.0,
    g: 92.0 / 255.0,
    b: 85.0 / 255.0,
    a: 1.0,
};
const ACCENT_DARK: Color = Color {
    r: 30.0 / 255.0,
    g: 74.0 / 255.0,
    b: 68.0 / 255.0,
    a: 1.0,
};
const TEXT_MUTED: Color = Color {
    r: 120.0 / 255.0,
    g: 120.0 / 255.0,
    b: 120.0 / 255.0,
    a: 1.0,
};
const BG_CANVAS: Color = Color {
    r: 243.0 / 255.0,
    g: 239.0 / 255.0,
    b: 232.0 / 255.0,
    a: 1.0,
};
const BG_PANEL: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const BG_HEADER: Color = Color {
    r: 245.0 / 255.0,
    g: 241.0 / 255.0,
    b: 235.0 / 255.0,
    a: 1.0,
};

pub fn run_ui() -> Result<()> {
    let settings = Settings {
        window: iced::window::Settings {
            size: iced::Size::new(1100.0, 720.0),
            min_size: Some(iced::Size::new(900.0, 600.0)),
            ..Default::default()
        },
        ..Settings::default()
    };
    NotesUi::run(settings).map_err(|err| anyhow!(err.to_string()))
}

struct NotesUi {
    app: Option<NotesApp>,
    summaries: Vec<NoteSummary>,
    selected_slug: Option<String>,
    editor: text_editor::Content,
    loaded_text: String,
    new_title: String,
    status_message: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    NewTitleChanged(String),
    CreateNote,
    SelectNote(String),
    EditorAction(text_editor::Action),
    SaveNote,
}

impl Application for NotesUi {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let mut ui = match NotesApp::load() {
            Ok(mut app) => {
                let _ = app.snapshot_all_changes();
                let summaries = app.note_summaries();
                Self {
                    app: Some(app),
                    summaries,
                    selected_slug: None,
                    editor: text_editor::Content::new(),
                    loaded_text: String::new(),
                    new_title: String::new(),
                    status_message: None,
                    error_message: None,
                }
            }
            Err(err) => Self {
                app: None,
                summaries: Vec::new(),
                selected_slug: None,
                editor: text_editor::Content::new(),
                loaded_text: String::new(),
                new_title: String::new(),
                status_message: None,
                error_message: Some(err.to_string()),
            },
        };

        let first_slug = ui.summaries.first().map(|note| note.slug.clone());
        if let Some(slug) = first_slug {
            if let Err(err) = ui.load_note(&slug) {
                ui.error_message = Some(err.to_string());
            }
        }

        (ui, Command::none())
    }

    fn title(&self) -> String {
        "Notes".to_string()
    }

    fn theme(&self) -> Theme {
        Theme::Light
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        let result = match message {
            Message::NewTitleChanged(value) => {
                self.new_title = value;
                Ok(())
            }
            Message::CreateNote => self.create_note(),
            Message::SelectNote(slug) => self.select_note(&slug),
            Message::EditorAction(action) => {
                self.editor.perform(action);
                Ok(())
            }
            Message::SaveNote => self.save_current("Saved").map(|_| ()),
        };

        if let Err(err) = result {
            self.error_message = Some(err.to_string());
        }

        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.app.is_none() {
            let error = self
                .error_message
                .as_deref()
                .unwrap_or("Unable to start notes.");
            return container(
                column![
            text("Notes").size(32),
            text(error).size(14).style(Color {
                r: 176.0 / 255.0,
                g: 69.0 / 255.0,
                b: 69.0 / 255.0,
                a: 1.0,
            }),
                ]
                .spacing(16)
                .align_items(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .into();
        }

        let header = self.header();
        let list_panel = self.list_panel();
        let editor = self.editor_panel();

        column![header, row![list_panel, editor].height(Length::Fill)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl NotesUi {
    fn app_mut(&mut self) -> Result<&mut NotesApp> {
        self.app
            .as_mut()
            .ok_or_else(|| anyhow!("Notes app unavailable"))
    }

    fn refresh_summaries(&mut self) {
        if let Some(app) = self.app.as_ref() {
            self.summaries = app.note_summaries();
        }
    }

    fn load_note(&mut self, slug: &str) -> Result<()> {
        let app = self.app_mut()?;
        let content = app.read_working_content(slug)?;
        self.editor = text_editor::Content::with_text(&content);
        self.loaded_text = content;
        self.selected_slug = Some(slug.to_string());
        self.status_message = None;
        self.error_message = None;
        Ok(())
    }

    fn create_note(&mut self) -> Result<()> {
        let title = if self.new_title.trim().is_empty() {
            None
        } else {
            Some(self.new_title.trim().to_string())
        };
        let app = self.app_mut()?;
        let (slug, _) = app.create_note_with_slug(title)?;
        app.save()?;
        self.new_title.clear();
        self.refresh_summaries();
        self.load_note(&slug)?;
        self.status_message = Some("Created".to_string());
        self.error_message = None;
        Ok(())
    }

    fn save_current(&mut self, label: &str) -> Result<bool> {
        let Some(slug) = self.selected_slug.clone() else {
            return Ok(false);
        };
        let content = self.editor.text();
        if content == self.loaded_text {
            return Ok(false);
        }
        let app = self.app_mut()?;
        app.write_working_content(&slug, &content)?;
        let _ = app.snapshot_if_changed(&slug)?;
        app.save()?;
        self.loaded_text = content;
        self.status_message = Some(label.to_string());
        self.error_message = None;
        self.refresh_summaries();
        Ok(true)
    }

    fn select_note(&mut self, slug: &str) -> Result<()> {
        let _ = self.save_current("Auto-saved");
        self.load_note(slug)
    }

    fn header(&self) -> Element<'_, Message> {
        let status = if let Some(error) = self.error_message.as_ref() {
            text(error)
                .size(13)
                .style(Color {
                    r: 176.0 / 255.0,
                    g: 69.0 / 255.0,
                    b: 69.0 / 255.0,
                    a: 1.0,
                })
        } else if let Some(message) = self.status_message.as_ref() {
            text(message).size(13).style(ACCENT)
        } else {
            text("").size(13)
        };

        let can_save = self
            .selected_slug
            .is_some()
            && self.editor.text() != self.loaded_text;

        let save_button = if can_save {
            button(text("Save").size(14).style(Color::WHITE))
                .on_press(Message::SaveNote)
                .style(iced::theme::Button::custom(PrimaryButton))
        } else {
            button(text("Save").size(14).style(Color::WHITE))
                .style(iced::theme::Button::custom(DisabledButton))
        };

        container(
            row![
                column![
                    text("Notes").size(26).style(ACCENT),
                    text("Minimal studio for quick thoughts.")
                        .size(13)
                        .style(TEXT_MUTED),
                ]
                .spacing(2),
                row![status, save_button]
                    .spacing(12)
                    .align_items(Alignment::Center)
            ]
            .spacing(20)
            .align_items(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(18)
        .style(|_theme: &Theme| container::Appearance {
            text_color: None,
            background: Some(iced::Background::Color(BG_HEADER)),
            border: iced::Border::default(),
            shadow: iced::Shadow::default(),
        })
        .into()
    }

    fn list_panel(&self) -> Element<'_, Message> {
        let mut list: Column<Message> = column![text("Library").size(14).style(TEXT_MUTED)]
            .spacing(8)
            .align_items(Alignment::Start);

        list = list.push(
            text_input("New note title", &self.new_title)
                .on_input(Message::NewTitleChanged)
                .on_submit(Message::CreateNote)
                .padding(8),
        );

        list = list.push(
            button(text("Create").size(14).style(Color::WHITE))
                .on_press(Message::CreateNote)
                .style(iced::theme::Button::custom(PrimaryButton))
                .padding(8),
        );

        list = list.push(text("Notes").size(14).style(ACCENT));

        let mut notes_column = column![].spacing(12);
        for summary in &self.summaries {
            let selected = self
                .selected_slug
                .as_deref()
                .map(|slug| slug == summary.slug.as_str())
                .unwrap_or(false);
            let title_style = if selected {
                text(&summary.title).size(16).style(ACCENT_DARK)
            } else {
                text(&summary.title).size(16)
            };
            let note_button = button(title_style)
                .on_press(Message::SelectNote(summary.slug.clone()))
                .style(iced::theme::Button::custom(NoteButton { selected }));

            let meta = format!(
                "v{}  |  {}",
                summary.current_version,
                summary.updated_at.format("%Y-%m-%d %H:%M")
            );
            notes_column = notes_column.push(
                column![
                    note_button,
                    text(meta).size(11).style(TEXT_MUTED)
                ]
                .spacing(4),
            );
        }

        list = list.push(scrollable(notes_column).height(Length::Fill));

        container(list)
            .width(Length::Fixed(280.0))
            .height(Length::Fill)
            .padding(16)
            .style(|_theme: &Theme| container::Appearance {
                text_color: None,
                background: Some(iced::Background::Color(BG_PANEL)),
                border: iced::Border::default(),
                shadow: iced::Shadow::default(),
            })
            .into()
    }

    fn editor_panel(&self) -> Element<'_, Message> {
        let header = if let Some(slug) = self.selected_slug.as_ref() {
            if let Some(summary) = self.summaries.iter().find(|note| &note.slug == slug) {
                column![
                    text(&summary.title).size(22),
                    text(format!(
                        "id: {}  |  {} versions",
                        summary.slug, summary.versions
                    ))
                    .size(12)
                    .style(TEXT_MUTED)
                ]
                .spacing(4)
            } else {
                column![text("Untitled").size(22)].spacing(4)
            }
        } else {
            column![text("Create a note to get started.")
                .size(16)
                .style(TEXT_MUTED)]
            .spacing(4)
        };

        let editor = if self.selected_slug.is_some() {
            text_editor(&self.editor)
                .on_action(Message::EditorAction)
                .height(Length::Fill)
        } else {
            text_editor(&self.editor).height(Length::Fill)
        };

        container(
            column![header, editor]
                .spacing(16)
                .align_items(Alignment::Start),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(24)
        .style(|_theme: &Theme| container::Appearance {
            text_color: None,
            background: Some(iced::Background::Color(BG_CANVAS)),
            border: iced::Border::default(),
            shadow: iced::Shadow::default(),
        })
        .into()
    }
}

struct PrimaryButton;

impl button::StyleSheet for PrimaryButton {
    type Style = Theme;

    fn active(&self, _style: &Theme) -> button::Appearance {
        button::Appearance {
            background: Some(iced::Background::Color(ACCENT)),
            border: iced::Border {
                radius: 6.0.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
            shadow_offset: iced::Vector::new(0.0, 0.0),
            text_color: Color::WHITE,
            shadow: iced::Shadow::default(),
        }
    }

    fn hovered(&self, style: &Theme) -> button::Appearance {
        let mut appearance = self.active(style);
        appearance.background = Some(iced::Background::Color(ACCENT_DARK));
        appearance
    }
}

struct DisabledButton;

impl button::StyleSheet for DisabledButton {
    type Style = Theme;

    fn active(&self, _style: &Theme) -> button::Appearance {
        button::Appearance {
            background: Some(iced::Background::Color(Color {
                r: 190.0 / 255.0,
                g: 192.0 / 255.0,
                b: 190.0 / 255.0,
                a: 1.0,
            })),
            border: iced::Border {
                radius: 6.0.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
            shadow_offset: iced::Vector::new(0.0, 0.0),
            text_color: Color {
                r: 240.0 / 255.0,
                g: 240.0 / 255.0,
                b: 240.0 / 255.0,
                a: 1.0,
            },
            shadow: iced::Shadow::default(),
        }
    }
}

struct NoteButton {
    selected: bool,
}

impl button::StyleSheet for NoteButton {
    type Style = Theme;

    fn active(&self, _style: &Theme) -> button::Appearance {
        let background = if self.selected {
            Some(iced::Background::Color(Color {
                r: 236.0 / 255.0,
                g: 232.0 / 255.0,
                b: 226.0 / 255.0,
                a: 1.0,
            }))
        } else {
            Some(iced::Background::Color(Color::TRANSPARENT))
        };
        button::Appearance {
            background,
            border: iced::Border {
                radius: 6.0.into(),
                width: 1.0,
                color: if self.selected {
                    Color {
                        r: 214.0 / 255.0,
                        g: 208.0 / 255.0,
                        b: 200.0 / 255.0,
                        a: 1.0,
                    }
                } else {
                    Color::TRANSPARENT
                },
            },
            shadow_offset: iced::Vector::new(0.0, 0.0),
            text_color: Color::from_rgb8(20, 20, 20),
            shadow: iced::Shadow::default(),
        }
    }

    fn hovered(&self, _style: &Theme) -> button::Appearance {
        let background = Some(iced::Background::Color(Color {
            r: 236.0 / 255.0,
            g: 232.0 / 255.0,
            b: 226.0 / 255.0,
            a: 1.0,
        }));
        button::Appearance {
            background,
            border: iced::Border {
                radius: 6.0.into(),
                width: 1.0,
                color: Color {
                    r: 214.0 / 255.0,
                    g: 208.0 / 255.0,
                    b: 200.0 / 255.0,
                    a: 1.0,
                },
            },
            shadow_offset: iced::Vector::new(0.0, 0.0),
            text_color: Color::from_rgb8(20, 20, 20),
            shadow: iced::Shadow::default(),
        }
    }
}

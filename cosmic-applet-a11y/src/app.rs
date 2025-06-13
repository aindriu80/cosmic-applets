// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use crate::{
    backend::{
        self,
        dbus::{DBusRequest, DBusUpdate},
        wayland::{AccessibilityEvent, AccessibilityRequest, WaylandUpdate},
    },
    fl,
};
use cosmic::{
    app,
    applet::{
        menu_button, padded_control,
        token::subscription::{activation_token_subscription, TokenRequest, TokenUpdate},
    },
    cctk::sctk::reexports::calloop::channel,
    cosmic_theme::Spacing,
    iced::{
        platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
        window, Length, Subscription,
    },
    surface, theme,
    widget::{divider, text, Column},
    Element, Task,
};
use cosmic_protocols::a11y::v1::client::cosmic_a11y_manager_v1::Filter;
use cosmic_time::{anim, chain, id, once_cell::sync::Lazy, Instant, Timeline};
use tokio::sync::mpsc::UnboundedSender;

static READER_TOGGLE: Lazy<id::Toggler> = Lazy::new(id::Toggler::unique);
static MAGNIFIER_TOGGLE: Lazy<id::Toggler> = Lazy::new(id::Toggler::unique);
static INVERT_COLORS_TOGGLE: Lazy<id::Toggler> = Lazy::new(id::Toggler::unique);

pub fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<CosmicA11yApplet>(())
}

#[derive(Clone, Default)]
struct CosmicA11yApplet {
    core: cosmic::app::Core,
    reader_enabled: bool,
    magnifier_enabled: bool,
    inverted_colors_enabled: bool,
    popup: Option<window::Id>,
    dbus_sender: Option<UnboundedSender<DBusRequest>>,
    wayland_sender: Option<channel::SyncSender<AccessibilityRequest>>,
    wayland_protocol_version: Option<u32>,
    timeline: Timeline,
    token_tx: Option<channel::Sender<TokenRequest>>,
}

#[derive(Debug, Clone)]
enum Message {
    TogglePopup,
    CloseRequested(window::Id),
    ScreenReaderEnabled(chain::Toggler, bool),
    MagnifierEnabled(chain::Toggler, bool),
    InvertedColorsEnabled(chain::Toggler, bool),
    Frame(Instant),
    Token(TokenUpdate),
    OpenSettings,
    DBusUpdate(DBusUpdate),
    WaylandUpdate(WaylandUpdate),
    Surface(surface::Action),
}

impl cosmic::Application for CosmicA11yApplet {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = "com.system76.CosmicAppletA11y";

    fn init(core: cosmic::app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        (
            Self {
                core,
                token_tx: None,

                ..Default::default()
            },
            Task::none(),
        )
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message> {
        match message {
            Message::Frame(now) => self.timeline.now(now),
            Message::ScreenReaderEnabled(chain, enabled) => {
                if let Some(tx) = &self.dbus_sender {
                    self.timeline.set_chain(chain).start();
                    self.reader_enabled = enabled;
                    let _ = tx.send(DBusRequest::Status(enabled));
                } else {
                    self.reader_enabled = false;
                }
            }
            Message::MagnifierEnabled(chain, enabled) => {
                if let Some(tx) = &self.wayland_sender {
                    self.timeline.set_chain(chain).start();
                    self.magnifier_enabled = enabled;
                    let _ = tx.send(AccessibilityRequest::Magnifier(enabled));
                } else {
                    self.magnifier_enabled = false;
                }
            }
            Message::InvertedColorsEnabled(chain, enabled) => {
                if let Some(tx) = &self.wayland_sender {
                    self.timeline.set_chain(chain).start();
                    self.inverted_colors_enabled = enabled;
                    let _ = tx.send(AccessibilityRequest::ScreenFilter {
                        inverted: enabled,
                        filter: Filter::Unknown,
                    });
                } else {
                    self.inverted_colors_enabled = false;
                }
            }
            Message::TogglePopup => {
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                } else {
                    self.timeline = Timeline::new();

                    let new_id = window::Id::unique();
                    self.popup.replace(new_id);

                    let popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        Some((1, 1)),
                        None,
                        None,
                    );

                    return get_popup(popup_settings);
                }
            }
            Message::CloseRequested(id) => {
                if Some(id) == self.popup {
                    self.popup = None;
                }
            }
            Message::OpenSettings => {
                let exec = "cosmic-settings accessibility".to_string();
                if let Some(tx) = self.token_tx.as_ref() {
                    let _ = tx.send(TokenRequest {
                        app_id: Self::APP_ID.to_string(),
                        exec,
                    });
                } else {
                    tracing::error!("Wayland tx is None");
                };
            }
            Message::Token(u) => match u {
                TokenUpdate::Init(tx) => {
                    self.token_tx = Some(tx);
                }
                TokenUpdate::Finished => {
                    self.token_tx = None;
                }
                TokenUpdate::ActivationToken { token, .. } => {
                    let mut cmd = std::process::Command::new("cosmic-settings");
                    cmd.arg("accessibility");
                    if let Some(token) = token {
                        cmd.env("XDG_ACTIVATION_TOKEN", &token);
                        cmd.env("DESKTOP_STARTUP_ID", &token);
                    }
                    tokio::spawn(cosmic::process::spawn(cmd));
                }
            },
            Message::DBusUpdate(update) => match update {
                DBusUpdate::Error(err) => {
                    tracing::error!("{err}");
                    let _ = self.dbus_sender.take();
                    self.reader_enabled = false;
                }
                DBusUpdate::Status(enabled) => {
                    self.reader_enabled = enabled;
                }
                DBusUpdate::Init(enabled, tx) => {
                    self.reader_enabled = enabled;
                    self.dbus_sender = Some(tx);
                }
            },
            Message::WaylandUpdate(update) => match update {
                WaylandUpdate::Errored => {
                    tracing::error!("Wayland error");
                    let _ = self.wayland_sender.take();
                    self.wayland_protocol_version = None;
                    self.magnifier_enabled = false;
                    self.inverted_colors_enabled = false;
                }
                WaylandUpdate::State(AccessibilityEvent::Bound(ver)) => {
                    self.wayland_protocol_version = Some(ver);
                }
                WaylandUpdate::State(AccessibilityEvent::Magnifier(enabled)) => {
                    self.magnifier_enabled = enabled;
                }
                WaylandUpdate::State(AccessibilityEvent::ScreenFilter { inverted, .. }) => {
                    self.inverted_colors_enabled = inverted;
                }
                WaylandUpdate::Started(tx) => {
                    self.wayland_sender = Some(tx);
                }
            },
            Message::Surface(a) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(a),
                ));
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        self.core
            .applet
            .icon_button("preferences-desktop-accessibility-symbolic")
            .on_press_down(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: window::Id) -> Element<Message> {
        let Spacing {
            space_xxs, space_s, ..
        } = theme::active().cosmic().spacing;

        let reader_toggle = padded_control(
            anim!(
                READER_TOGGLE,
                &self.timeline,
                fl!("screen-reader"),
                self.reader_enabled,
                Message::ScreenReaderEnabled,
            )
            .text_size(14)
            .width(Length::Fill),
        );
        let magnifier_toggle = padded_control(
            anim!(
                MAGNIFIER_TOGGLE,
                &self.timeline,
                fl!("magnifier"),
                self.magnifier_enabled,
                Message::MagnifierEnabled,
            )
            .text_size(14)
            .width(Length::Fill),
        );
        let invert_colors_toggle = padded_control(
            anim!(
                INVERT_COLORS_TOGGLE,
                &self.timeline,
                fl!("invert-colors"),
                self.inverted_colors_enabled,
                Message::InvertedColorsEnabled,
            )
            .text_size(14)
            .width(Length::Fill),
        );

        let content_list = Column::with_capacity(5)
            .push(reader_toggle)
            .push_maybe(
                self.wayland_protocol_version
                    .is_some()
                    .then_some(magnifier_toggle),
            )
            .push_maybe(
                self.wayland_protocol_version
                    .is_some_and(|ver| ver >= 2)
                    .then_some(invert_colors_toggle),
            )
            .push(padded_control(divider::horizontal::default()).padding([space_xxs, space_s]))
            .push(menu_button(text::body(fl!("settings"))).on_press(Message::OpenSettings))
            .padding([8, 0]);

        self.core.applet.popup_container(content_list).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            backend::dbus::subscription().map(Message::DBusUpdate),
            backend::wayland::a11y_subscription().map(Message::WaylandUpdate),
            self.timeline
                .as_subscription()
                .map(|(_, now)| Message::Frame(now)),
            activation_token_subscription(0).map(Message::Token),
        ])
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}

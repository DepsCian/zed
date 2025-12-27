use chrono::Utc;
use gpui::{ClipboardItem, Context};
use ui::{ButtonLike, ConfiguredApiCard, prelude::*};

use super::auth::{AuthStatus, DeviceFlowPrompt};
use super::config::{AVAILABLE_REGIONS, AWS_BUILDER_ID_URL};
use super::configuration_view::KiroConfigurationView;

impl KiroConfigurationView {
    pub fn render_instructions(&self) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(Label::new(
                "Kiro AI provides AI-powered coding assistance through AWS Builder ID authentication.",
            ))
            .child(Label::new(
                "Sign in with your AWS Builder ID to access Kiro's AI features.",
            ))
    }

    pub fn render_region_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_region = self.selected_region.clone();

        v_flex()
            .gap_1()
            .child(Label::new("Region").size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .gap_2()
                    .children(AVAILABLE_REGIONS.iter().map(|(region, label)| {
                        let is_selected = current_region == *region;
                        let region_str = region.to_string();

                        Button::new(SharedString::from(*region), *label)
                            .style(if is_selected {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Outlined
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_region(region_str.clone(), cx);
                            }))
                    })),
            )
    }

    pub fn render_device_code_ui(&self, prompt: &DeviceFlowPrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let user_code = prompt.user_code.clone();
        let verification_uri = prompt.verification_uri.clone();
        let expires_at = prompt.expires_at;

        let remaining_seconds = (expires_at - Utc::now()).num_seconds().max(0);
        let minutes = remaining_seconds / 60;
        let seconds = remaining_seconds % 60;

        let copied = cx
            .read_from_clipboard()
            .map(|item| item.text().as_ref() == Some(&user_code))
            .unwrap_or(false);

        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Enter this code on AWS:").color(Color::Muted))
                    .child(
                        ButtonLike::new("copy-code")
                            .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                            .child(
                                h_flex()
                                    .w_full()
                                    .px_3()
                                    .py_2()
                                    .justify_between()
                                    .child(
                                        Label::new(user_code.clone())
                                            .size(LabelSize::Large)
                                            .weight(gpui::FontWeight::BOLD),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(Icon::new(IconName::Copy).size(IconSize::Small))
                                            .child(Label::new(if copied { "Copied!" } else { "Copy" })),
                                    ),
                            )
                            .on_click({
                                let code = user_code.clone();
                                move |_, _window: &mut Window, cx: &mut App| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                }
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open-aws", "Open AWS")
                            .style(ButtonStyle::Outlined)
                            .icon(IconName::ArrowUpRight)
                            .icon_size(IconSize::Small)
                            .icon_position(IconPosition::End)
                            .on_click({
                                let uri = verification_uri.clone();
                                move |_, _, cx| cx.open_url(&uri)
                            }),
                    )
                    .child(
                        Button::new("cancel-sign-in", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_sign_in(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::CountdownTimer).size(IconSize::Small).color(Color::Muted))
                    .child(
                        Label::new(format!("Expires in {}:{:02}", minutes, seconds))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
    }

    pub fn render_sign_in_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("sign-in", "Sign in with AWS Builder ID")
            .full_width()
            .style(ButtonStyle::Outlined)
            .icon(IconName::Person)
            .icon_position(IconPosition::Start)
            .icon_size(IconSize::Small)
            .on_click(cx.listener(|this, _, _, cx| {
                this.sign_in(cx);
            }))
    }

    pub fn render_error(&self, message: &str, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::Warning).color(Color::Error))
                    .child(Label::new(format!("Error: {}", message)).color(Color::Error)),
            )
            .child(self.render_sign_in_button(cx))
    }
}

impl Render for KiroConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let auth_status = self.state.read(cx).auth_status.clone();
        let is_authenticated = self.state.read(cx).is_authenticated();

        v_flex()
            .gap_3()
            .child(self.render_instructions())
            .child(self.render_region_selector(cx))
            .child(match auth_status {
                AuthStatus::SignedOut => {
                    self.render_sign_in_button(cx).into_any_element()
                }
                AuthStatus::SigningIn { prompt } => {
                    self.render_device_code_ui(&prompt, cx).into_any_element()
                }
                AuthStatus::Authenticated => {
                    if is_authenticated {
                        ConfiguredApiCard::new("Authenticated with AWS Builder ID")
                            .button_label("Sign Out")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sign_out(cx);
                            }))
                            .into_any_element()
                    } else {
                        self.render_sign_in_button(cx).into_any_element()
                    }
                }
                AuthStatus::Error(msg) => {
                    self.render_error(&msg, cx).into_any_element()
                }
            })
            .child(
                Button::new("aws-builder-id-info", "Learn about AWS Builder ID")
                    .style(ButtonStyle::Subtle)
                    .icon(IconName::ArrowUpRight)
                    .icon_size(IconSize::XSmall)
                    .icon_color(Color::Muted)
                    .on_click(move |_, _, cx| cx.open_url(AWS_BUILDER_ID_URL)),
            )
    }
}

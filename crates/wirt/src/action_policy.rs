//! Host-side policy for action batches returned by untrusted plugins.

use crate::model::PluginAction;

pub const MAX_TOAST_ACTIONS: usize = 4;
pub const MAX_REQUEST_FETCH_ACTIONS: usize = 2;
pub const MAX_TOAST_MESSAGE_BYTES: usize = 1024;
pub const MAX_REQUEST_FETCH_KEY_BYTES: usize = 512;
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_PAGE_NAME_BYTES: usize = 512;
pub const MAX_LIGHTBOX_IMAGES: usize = 32;
pub const MAX_LIGHTBOX_KEY_BYTES: usize = 2048;
pub const MAX_LIGHTBOX_URL_BYTES: usize = 4096;
pub const MAX_LIGHTBOX_TITLE_BYTES: usize = 512;

fn within(value: &str, limit: usize) -> bool {
    value.len() <= limit
}

#[derive(Debug)]
pub struct BoundedPluginActions {
    pub actions: Vec<PluginAction>,
    pub limited: bool,
}

/// Bound guest-controlled actions before they enter a host queue or perform a
/// side effect. Repeated state-setting actions are last-wins; notification and
/// network actions have deliberately small quotas.
pub fn bound_plugin_actions(actions: Vec<PluginAction>) -> Vec<PluginAction> {
    bound_plugin_actions_with_status(actions).actions
}

/// Apply [`bound_plugin_actions`] while retaining whether any guest input was
/// rejected, truncated, or coalesced.
pub fn bound_plugin_actions_with_status(actions: Vec<PluginAction>) -> BoundedPluginActions {
    let mut bounded = Vec::new();
    let mut limited = false;
    let mut toast_count = 0;
    let mut fetch_count = 0;
    let mut refresh = None;
    let mut close_dialog = None;
    let mut clipboard = None;
    let mut lightbox = None;
    let mut page_name = None;

    for (index, action) in actions.into_iter().enumerate() {
        match action {
            PluginAction::None => {}
            PluginAction::ShowToast { message, level }
                if toast_count < MAX_TOAST_ACTIONS && within(&message, MAX_TOAST_MESSAGE_BYTES) =>
            {
                toast_count += 1;
                bounded.push((index, PluginAction::ShowToast { message, level }));
            }
            PluginAction::ShowToast { .. } => limited = true,
            PluginAction::RequestFetch { key }
                if fetch_count < MAX_REQUEST_FETCH_ACTIONS
                    && within(&key, MAX_REQUEST_FETCH_KEY_BYTES) =>
            {
                fetch_count += 1;
                bounded.push((index, PluginAction::RequestFetch { key }));
            }
            PluginAction::RequestFetch { .. } => limited = true,
            PluginAction::RefreshPanel { .. } => {
                // Refresh is currently a single dirty bit. Do not retain a
                // guest extension-point string that the host does not use.
                if refresh.is_some() {
                    limited = true;
                } else {
                    refresh = Some((
                        index,
                        PluginAction::RefreshPanel {
                            extension_point: String::new(),
                        },
                    ));
                }
            }
            PluginAction::CloseDialog => {
                if close_dialog.is_some() {
                    limited = true;
                } else {
                    close_dialog = Some((index, PluginAction::CloseDialog));
                }
            }
            PluginAction::CopyToClipboard { text } if within(&text, MAX_CLIPBOARD_TEXT_BYTES) => {
                limited |= clipboard
                    .replace((index, PluginAction::CopyToClipboard { text }))
                    .is_some();
            }
            PluginAction::CopyToClipboard { .. } => limited = true,
            PluginAction::SetPageDisplayName { name } if within(&name, MAX_PAGE_NAME_BYTES) => {
                limited |= page_name
                    .replace((index, PluginAction::SetPageDisplayName { name }))
                    .is_some();
            }
            PluginAction::SetPageDisplayName { .. } => limited = true,
            PluginAction::OpenLightbox {
                images,
                start_index,
                title,
            } => {
                let submitted_image_count = images.len();
                let submitted_start_index = start_index;
                let images: Vec<_> = images
                    .into_iter()
                    .filter(|(key, url)| {
                        within(key, MAX_LIGHTBOX_KEY_BYTES)
                            && url
                                .as_deref()
                                .is_none_or(|url| within(url, MAX_LIGHTBOX_URL_BYTES))
                    })
                    .take(MAX_LIGHTBOX_IMAGES)
                    .collect();
                limited |= images.len() != submitted_image_count;
                let title = match title {
                    Some(title) if within(&title, MAX_LIGHTBOX_TITLE_BYTES) => Some(title),
                    Some(_) => {
                        limited = true;
                        None
                    }
                    None => None,
                };
                if !images.is_empty() {
                    let start_index = start_index.min(images.len() - 1);
                    limited |= start_index != submitted_start_index;
                    limited |= lightbox
                        .replace((
                            index,
                            PluginAction::OpenLightbox {
                                images,
                                start_index,
                                title,
                            },
                        ))
                        .is_some();
                } else {
                    limited = true;
                }
            }
        }
    }

    bounded.extend(refresh);
    bounded.extend(close_dialog);
    bounded.extend(clipboard);
    bounded.extend(lightbox);
    bounded.extend(page_name);
    bounded.sort_by_key(|(index, _)| *index);
    BoundedPluginActions {
        actions: bounded.into_iter().map(|(_, action)| action).collect(),
        limited,
    }
}

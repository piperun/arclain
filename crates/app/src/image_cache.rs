//! Cache-key and byte-access rules shared by ordinary host images and the
//! optional plugin-host image facade.

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};

const MIN_FETCHED_IMAGE_BYTES: usize = 1000;

/// Bytes for one image reference, plus whether serving them needed the
/// network.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ImageBytesDto {
    pub bytes: Vec<u8>,
    pub served_from_cache: bool,
}

#[cfg(feature = "plugin-host")]
pub const MAX_PLUGIN_IMAGE_BYTES: u32 = 16 * 1024 * 1024;
const PLUGIN_IMAGE_CACHE_KEY_PREFIX: &str = "plugin-image:";

#[cfg(feature = "plugin-host")]
pub(crate) fn encode_plugin_image_cache_key(plugin_id: &str, raw_key: &str) -> String {
    format!("{PLUGIN_IMAGE_CACHE_KEY_PREFIX}{plugin_id}:{raw_key}")
}

#[cfg(feature = "plugin-host")]
pub(crate) fn decode_plugin_image_cache_key(cache_key: &str) -> Option<(&str, &str)> {
    cache_key
        .strip_prefix(PLUGIN_IMAGE_CACHE_KEY_PREFIX)?
        .split_once(':')
}

pub fn is_plugin_image_key(cache_key: &str) -> bool {
    cache_key.starts_with(PLUGIN_IMAGE_CACHE_KEY_PREFIX)
}

#[cfg(feature = "plugin-host")]
pub fn plugin_image_key_owner(cache_key: &str) -> Option<&str> {
    decode_plugin_image_cache_key(cache_key).map(|(plugin_id, _)| plugin_id)
}

pub const MAX_HOST_IMAGE_BYTES: u32 = 50 * 1024 * 1024;

const _: () = assert!(
    MAX_HOST_IMAGE_BYTES as usize == arclain_data::DEFAULT_MAX_RESOURCE_SIZE_BYTES,
    "MAX_HOST_IMAGE_BYTES must preserve the content cache's default read ceiling",
);

/// Resolves the host-namespace raw key `cache_key` addresses, refusing
/// anything that could name a row outside the host namespace.
///
/// The refusal is the host half of the namespace boundary, and it is a
/// security property rather than tidiness: without it, every host image
/// method would be a second, unauthorized door into a plugin's cache
/// namespace. It closes both this facade's `plugin-image:{owner}:{key}`
/// encoding and the storage layer's own scoped-key encoding.
pub(crate) fn host_image_key(cache_key: &str) -> Result<&str, ApplicationError> {
    let refuse = |summary: &str| {
        Err(
            ApplicationError::new(ApplicationErrorKind::PermissionDenied, summary)
                .with_recoverability(Recoverability::Fatal)
                .with_field("cache_key"),
        )
    };
    if is_plugin_image_key(cache_key) {
        return refuse("cache key belongs to a plugin image namespace, not the host");
    }
    if arclain_data::CacheOwner::from_scoped_key(cache_key).is_some()
        || cache_key.starts_with(CACHE_SCOPED_KEY_SENTINEL)
    {
        return refuse("cache key names a storage-scoped cache row, not a host image");
    }
    Ok(cache_key)
}

pub(crate) const CACHE_SCOPED_KEY_SENTINEL: char = '\u{1}';

#[cfg(feature = "plugin-host")]
pub(crate) fn oversized_image_error(
    summary: &str,
    actual: usize,
    limit: usize,
) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, summary)
        .with_diagnostic(format!("{actual} bytes exceeds the {limit}-byte limit"))
        .with_recoverability(Recoverability::Fatal)
}

pub(crate) fn read_host_image(
    content_cache: &arclain_data::ContentCache,
    cache_key: &str,
) -> Result<Vec<u8>, ApplicationError> {
    read_cached_host_image(content_cache, host_image_key(cache_key)?)?.ok_or_else(|| {
        ApplicationError::new(ApplicationErrorKind::NotFound, "host image not found")
            .with_recoverability(Recoverability::Fatal)
    })
}

pub(crate) fn read_cached_host_image(
    content_cache: &arclain_data::ContentCache,
    raw_key: &str,
) -> Result<Option<Vec<u8>>, ApplicationError> {
    content_cache
        .get_with_limit(raw_key, MAX_HOST_IMAGE_BYTES as usize)
        .map_err(|error| {
            ApplicationError::new(ApplicationErrorKind::Internal, "failed to read host image")
                .with_diagnostic(error.to_string())
                .with_recoverability(Recoverability::Fatal)
        })
}

pub(crate) fn discard_host_image(
    content_cache: &arclain_data::ContentCache,
    cache_key: &str,
) -> Result<bool, ApplicationError> {
    content_cache
        .remove(host_image_key(cache_key)?)
        .map_err(|error| {
            ApplicationError::new(
                ApplicationErrorKind::Internal,
                "failed to discard host image",
            )
            .with_diagnostic(error.to_string())
            .with_recoverability(Recoverability::Retry)
        })
}

/// Serves a host-owned cache key, fetching and caching its URL on a miss.
pub(crate) fn fetch_host_image(
    content_cache: &arclain_data::ContentCache,
    http: &arclain_network::AsyncHttpClient,
    cache_key: &str,
    url: &str,
    on_behalf_of_plugin: Option<&str>,
) -> Result<ImageBytesDto, ApplicationError> {
    let raw_key = host_image_key(cache_key)?;
    if let Some(bytes) = read_cached_host_image(content_cache, raw_key)? {
        return Ok(ImageBytesDto {
            bytes,
            served_from_cache: true,
        });
    }
    let bytes = fetch_display_image(
        http,
        on_behalf_of_plugin,
        url,
        MAX_HOST_IMAGE_BYTES as usize,
    )?;
    content_cache
        .put(
            raw_key,
            &bytes,
            arclain_core::CacheType::Screenshot,
            None,
            Some(url),
        )
        .map_err(|error| {
            ApplicationError::new(ApplicationErrorKind::Internal, "failed to cache host image")
                .with_diagnostic(error.to_string())
                .with_recoverability(Recoverability::Retry)
        })?;
    Ok(ImageBytesDto {
        bytes,
        served_from_cache: false,
    })
}

/// Fetches a display image while enforcing the byte ceiling during I/O.
pub(crate) fn fetch_display_image(
    http: &arclain_network::AsyncHttpClient,
    on_behalf_of_plugin: Option<&str>,
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, ApplicationError> {
    let fetched = match on_behalf_of_plugin {
        Some(plugin_id) => {
            http.blocking_get_response_for_plugin_with_limit(plugin_id, url, max_bytes)
        }
        None => http.blocking_get_response_with_limit(url, false, max_bytes),
    };
    let response = fetched.map_err(image_fetch_error)?;

    if response.status_code != 200 {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch returned an unexpected status",
        )
        .with_diagnostic(format!("HTTP status {}", response.status_code))
        .with_recoverability(Recoverability::Retry));
    }
    if !response
        .content_type
        .as_deref()
        .is_some_and(|content_type| content_type.starts_with("image/"))
    {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch returned a non-image content type",
        )
        .with_recoverability(Recoverability::Fatal));
    }
    if response.body.len() <= MIN_FETCHED_IMAGE_BYTES {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch returned too few bytes to be an image",
        )
        .with_diagnostic(format!(
            "{} bytes is at or below the {MIN_FETCHED_IMAGE_BYTES}-byte floor",
            response.body.len()
        ))
        .with_recoverability(Recoverability::Retry));
    }
    Ok(response.body)
}

fn image_fetch_error(error: arclain_network::HttpError) -> ApplicationError {
    match error {
        arclain_network::HttpError::ResponseTooLarge { limit } => ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch exceeded the maximum size",
        )
        .with_diagnostic(format!("response body exceeds the {limit}-byte limit"))
        .with_recoverability(Recoverability::Fatal),
        other => ApplicationError::new(ApplicationErrorKind::Backend, "image fetch failed")
            .with_diagnostic(other.to_string())
            .with_recoverability(Recoverability::Retry)
            .with_retryable(true),
    }
}

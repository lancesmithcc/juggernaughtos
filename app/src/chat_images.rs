//! Private raster attachments, owned by the conversation's user.
use crate::api::{ChatImage, MAX_CHAT_IMAGES, MAX_CHAT_IMAGE_BYTES};
use crate::server::{session_user, AppState};
use axum::extract::{Multipart, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jugg_ai::InputImage;
use jugg_db::sqlx::{self, SqliteConnection};
use serde_json::json;

pub const MAX_REQUEST_BYTES: usize = MAX_CHAT_IMAGES * MAX_CHAT_IMAGE_BYTES + 64 * 1024;
const MAX_PIXELS: u64 = 20_000_000;
const MAX_EDGE: u32 = 1024;
static IMAGE_DECODER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub struct Upload {
    name: String,
    png: Vec<u8>,
}

fn decode(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let invalid = "Choose a valid PNG, JPEG, WebP, or GIF image.";
    let format = image::guess_format(&bytes).map_err(|_| invalid)?;
    use image::ImageFormat::{Gif, Jpeg, Png, WebP};
    if !matches!(format, Png | Jpeg | WebP | Gif) {
        return Err(invalid.into());
    }
    let reader = || image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
    let (width, height) = reader().into_dimensions().map_err(|_| invalid)?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("Images can be up to 20 megapixels.".into());
    }
    let image = reader().decode().map_err(|_| invalid)?;
    // Bake camera orientation into JPEG pixels before removing metadata.
    let image = if format == Jpeg {
        use image::ImageDecoder;
        let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&bytes))
            .map_err(|_| invalid)?;
        let mut image = image;
        if let Ok(orientation) = decoder.orientation() {
            image.apply_orientation(orientation);
        }
        image
    } else {
        image
    };
    let image = if image.width().max(image.height()) > MAX_EDGE {
        image.thumbnail(MAX_EDGE, MAX_EDGE)
    } else {
        image
    };
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image.into_rgba8())
        .write_to(&mut output, Png)
        .map_err(|_| "Couldn't prepare the image.".to_string())?;
    Ok(output.into_inner())
}

fn problem(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

pub async fn upload(
    State(st): State<AppState>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Response {
    let Some(user) = session_user(&st, &headers).await else {
        return problem(StatusCode::UNAUTHORIZED, "Sign in to send images.");
    };
    // A cross-site form cannot supply this header. The admin has no CORS allowance.
    if headers.get("x-jugg-chat").and_then(|v| v.to_str().ok()) != Some("1") {
        return problem(StatusCode::FORBIDDEN, "Open the chat to send images.");
    }

    let result = async {
        let mut text = None;
        let mut conversation = None;
        let mut images = Vec::new();
        while let Some(mut field) = form
            .next_field()
            .await
            .map_err(|_| "Couldn't read the upload.")?
        {
            let (max, image_name) = match field.name() {
                Some("text") if text.is_none() => (20_000, None),
                Some("conversation_id") if conversation.is_none() => (64, None),
                Some("image") if images.len() < MAX_CHAT_IMAGES => {
                    let name: String = field
                        .file_name()
                        .unwrap_or("Image")
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or("Image")
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(120)
                        .collect();
                    (
                        MAX_CHAT_IMAGE_BYTES,
                        Some(if name.trim().is_empty() {
                            "Image".into()
                        } else {
                            name
                        }),
                    )
                }
                Some("image") => return Err("Attach up to four images per message.".into()),
                _ => return Err("Unexpected field in the upload.".into()),
            };
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| "Couldn't read the upload.")?
            {
                if bytes.len() + chunk.len() > max {
                    return Err(if image_name.is_some() {
                        "Images can be up to 10 MB each."
                    } else {
                        "The message is too long."
                    }
                    .into());
                }
                bytes.extend_from_slice(&chunk);
            }
            if let Some(name) = image_name {
                let permit = IMAGE_DECODER
                    .acquire()
                    .await
                    .map_err(|_| "Couldn't prepare the image.")?;
                let png = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    decode(bytes)
                })
                .await
                .map_err(|_| "Couldn't prepare the image.")??;
                images.push(Upload { name, png });
            } else {
                let value = String::from_utf8(bytes).map_err(|_| "Invalid message text.")?;
                if field.name() == Some("text") {
                    text = Some(value);
                } else {
                    conversation = Some(value);
                }
            }
        }
        crate::server::start_turn(&st, &user, conversation, text.unwrap_or_default(), images).await
    }
    .await;

    match result {
        Ok(ticket) => Json(ticket).into_response(),
        Err(message) => problem(StatusCode::BAD_REQUEST, message),
    }
}

pub async fn store(
    st: &AppState,
    db: &mut SqliteConnection,
    message: &str,
    uploads: Vec<Upload>,
) -> Result<Vec<ChatImage>, String> {
    let mut images = Vec::new();
    for (position, upload) in uploads.into_iter().enumerate() {
        let id = jugg_core::new_id();
        let image_enc = jugg_crypto::seal(
            &st.ring,
            &upload.png,
            format!("chat_images:image:{id}").as_bytes(),
        )
        .map_err(crate::server::err)?;
        let name_enc = jugg_crypto::seal(
            &st.ring,
            upload.name.as_bytes(),
            format!("chat_images:name:{id}").as_bytes(),
        )
        .map_err(crate::server::err)?;
        sqlx::query("INSERT INTO chat_images (id, message_id, position, image_enc, name_enc) VALUES (?,?,?,?,?)")
            .bind(&id).bind(message).bind(position as i64).bind(image_enc).bind(name_enc)
            .execute(&mut *db).await.map_err(crate::server::err)?;
        images.push(ChatImage {
            id,
            name: upload.name,
        });
    }
    Ok(images)
}

pub async fn recent(
    st: &AppState,
    conversation: &str,
    message: &str,
) -> Result<Vec<InputImage>, String> {
    // Bound the image context while keeping the full text history.
    let rows: Vec<(String, String, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT i.id, i.message_id, i.name_enc, i.image_enc
         FROM chat_images i JOIN messages m ON i.message_id = m.id
         WHERE m.conversation_id = ?
         AND (m.created_at, m.id) <= (SELECT created_at, id FROM messages WHERE id = ?)
         ORDER BY m.created_at DESC, m.id DESC, i.position LIMIT ?",
    )
    .bind(conversation)
    .bind(message)
    .bind(MAX_CHAT_IMAGES as i64)
    .fetch_all(&st.pool)
    .await
    .map_err(crate::server::err)?;
    rows.into_iter()
        .map(|(id, message_id, name_enc, image_enc)| {
            let name = jugg_crypto::open(
                &st.ring,
                &name_enc,
                format!("chat_images:name:{id}").as_bytes(),
            )
            .map_err(crate::server::err)?;
            let png = jugg_crypto::open(
                &st.ring,
                &image_enc,
                format!("chat_images:image:{id}").as_bytes(),
            )
            .map_err(crate::server::err)?;
            Ok(InputImage {
                name: String::from_utf8_lossy(&name).into_owned(),
                png,
                previous_turn: message_id != message,
            })
        })
        .collect()
}

pub async fn serve(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = session_user(&st, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let result: Result<Option<Vec<u8>>, _> = sqlx::query_scalar(
        "SELECT i.image_enc FROM chat_images i
         JOIN messages m ON i.message_id = m.id
         JOIN conversations c ON m.conversation_id = c.id
         WHERE i.id = ? AND c.user_id = ?",
    )
    .bind(&id)
    .bind(user)
    .fetch_optional(&st.pool)
    .await;
    match result {
        Ok(Some(enc)) => {
            match jugg_crypto::open(&st.ring, &enc, format!("chat_images:image:{id}").as_bytes()) {
                Ok(png) => (
                    [
                        (header::CONTENT_TYPE, "image/png"),
                        (header::CACHE_CONTROL, "private, no-store"),
                        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    ],
                    png,
                )
                    .into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploaded_raster_is_bounded_and_reencoded() {
        assert!(decode(b"<svg />".to_vec()).is_err());
        let mut original = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2048, 1536)
            .write_to(&mut original, image::ImageFormat::WebP)
            .unwrap();
        let png = decode(original.into_inner()).unwrap();
        assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png);
        let normalized = image::load_from_memory(&png).unwrap();
        assert_eq!((normalized.width(), normalized.height()), (1024, 768));
    }
}

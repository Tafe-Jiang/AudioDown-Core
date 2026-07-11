use axum::{
    body::Body,
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderValue, StatusCode, Uri,
    },
    response::Response,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct WebAssets;

pub async fn serve(uri: Uri) -> Response {
    let requested_path = uri.path().trim_start_matches('/');
    let asset_path = if requested_path.is_empty() {
        "index.html"
    } else {
        requested_path
    };

    if let Some(asset) = WebAssets::get(asset_path) {
        return asset_response(asset_path, asset.data.into_owned());
    }

    match WebAssets::get("index.html") {
        Some(index) => asset_response("index.html", index.data.into_owned()),
        None => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("embedded UI is unavailable"))
            .expect("static fallback response must be valid"),
    }
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(
            CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref())
                .expect("MIME types from mime_guess must be valid headers"),
        )
        .header(CACHE_CONTROL, cache_control)
        .body(Body::from(bytes))
        .expect("embedded asset response must be valid")
}

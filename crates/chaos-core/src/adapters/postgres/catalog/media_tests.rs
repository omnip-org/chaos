use super::*;

#[test]
fn product_metadata_pointer_is_set_and_removed_without_inline_media_bytes() {
    let segments = parse_json_pointer("/landing_page/hero/image").unwrap();
    let mut metadata = json!({
        "landing_page": {
            "hero": {
                "image": {
                    "crop": "cover",
                    "url": "https://stale.example/image.jpg",
                    "media_type": "image/jpeg"
                }
            }
        }
    });
    let asset_id = Uuid::now_v7();
    set_media_reference(&mut metadata, &segments, asset_id, "Hero").unwrap();

    assert_eq!(pointer_media_asset_id(&metadata, &segments), Some(asset_id));
    assert_eq!(metadata["landing_page"]["hero"]["image"]["crop"], "cover");
    assert!(clear_media_reference(&mut metadata, &segments));
    assert_eq!(
        metadata,
        json!({
            "landing_page": {
                "hero": {
                    "image": {"crop": "cover"}
                }
            }
        })
    );
}

#[test]
fn metadata_pointer_rejects_invalid_escape_sequences() {
    assert!(parse_json_pointer("/landing_page/~2image").is_err());
    assert!(parse_json_pointer("/landing_page//image").is_err());
    assert!(parse_json_pointer("landing_page/hero").is_err());
}

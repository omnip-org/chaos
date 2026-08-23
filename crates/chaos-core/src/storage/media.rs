use std::time::Duration;

use crate::{
    ApplicationError,
    ports::{MediaStorage, MediaUploadRequest, StoredMediaObject},
};
use async_trait::async_trait;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region},
    presigning::PresigningConfig,
    types::ChecksumMode,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chaos_domain::catalog::MediaDescriptor;
use time::OffsetDateTime;
use url::Url;

pub struct S3MediaStorage {
    client: Client,
    bucket: String,
    public_base_url: Url,
}

pub struct S3MediaStorageConfiguration {
    pub endpoint_url: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub force_path_style: bool,
    pub public_base_url: Url,
}

impl S3MediaStorage {
    pub fn new(configuration: S3MediaStorageConfiguration) -> anyhow::Result<Self> {
        if configuration.bucket.trim().is_empty() {
            anyhow::bail!("Media storage bucket must not be empty");
        }
        if configuration.public_base_url.scheme() != "https"
            || configuration.public_base_url.cannot_be_a_base()
            || configuration.public_base_url.query().is_some()
            || configuration.public_base_url.fragment().is_some()
        {
            anyhow::bail!(
                "Media public base URL must be an HTTPS base URL without query or fragment"
            );
        }
        let credentials = Credentials::new(
            configuration.access_key_id,
            configuration.secret_access_key,
            configuration.session_token,
            None,
            "chaos-media",
        );
        let mut builder = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(configuration.region))
            .credentials_provider(credentials)
            .force_path_style(configuration.force_path_style);
        if let Some(endpoint_url) = configuration.endpoint_url {
            builder = builder.endpoint_url(endpoint_url);
        }
        let mut public_base_url = configuration.public_base_url;
        if !public_base_url.path().ends_with('/') {
            public_base_url
                .path_segments_mut()
                .map_err(|()| anyhow::anyhow!("Media public base URL cannot be a base URL"))?
                .push("");
        }
        Ok(Self {
            client: Client::from_conf(builder.build()),
            bucket: configuration.bucket,
            public_base_url,
        })
    }
}

#[async_trait]
impl MediaStorage for S3MediaStorage {
    async fn prepare_upload(
        &self,
        object_key: &str,
        descriptor: &MediaDescriptor,
        valid_for: Duration,
        expires_at: OffsetDateTime,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        let checksum = STANDARD.encode(decode_digest(descriptor.sha256_hex())?);
        let configuration = PresigningConfig::expires_in(valid_for).map_err(|error| {
            ApplicationError::Unexpected(
                anyhow::Error::new(error).context("invalid Media upload expiration"),
            )
        })?;
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(object_key)
            .content_type(descriptor.media_type())
            .content_length(i64::try_from(descriptor.byte_size()).map_err(|_| {
                ApplicationError::Unexpected(anyhow::anyhow!("Media byte size is out of range"))
            })?)
            .checksum_sha256(checksum)
            .presigned(configuration)
            .await
            .map_err(storage_error)?;
        Ok(MediaUploadRequest {
            method: "PUT",
            url: request.uri().to_owned(),
            headers: request
                .headers()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            expires_at,
        })
    }

    async fn inspect(
        &self,
        object_key: &str,
    ) -> Result<Option<StoredMediaObject>, ApplicationError> {
        let result = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(object_key)
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await;
        let output = match result {
            Ok(output) => output,
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(storage_error(error)),
        };
        let media_type = output
            .content_type()
            .ok_or_else(|| malformed_object("content type"))?;
        let byte_size = u64::try_from(
            output
                .content_length()
                .ok_or_else(|| malformed_object("content length"))?,
        )
        .map_err(|_| malformed_object("content length"))?;
        let checksum = output
            .checksum_sha256()
            .ok_or_else(|| malformed_object("SHA-256 checksum"))?;
        let digest = STANDARD
            .decode(checksum)
            .map_err(|_| malformed_object("SHA-256 checksum"))?;
        if digest.len() != 32 {
            return Err(malformed_object("SHA-256 checksum"));
        }
        Ok(Some(StoredMediaObject {
            media_type: media_type.to_owned(),
            byte_size,
            sha256_hex: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        }))
    }

    fn public_url(&self, object_key: &str) -> Result<String, ApplicationError> {
        self.public_base_url
            .join(object_key)
            .map(|url| url.to_string())
            .map_err(|error| ApplicationError::Unexpected(error.into()))
    }
}

pub struct UnavailableMediaStorage;

#[async_trait]
impl MediaStorage for UnavailableMediaStorage {
    async fn prepare_upload(
        &self,
        _: &str,
        _: &MediaDescriptor,
        _: Duration,
        _: OffsetDateTime,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        Err(unavailable())
    }
    async fn inspect(&self, _: &str) -> Result<Option<StoredMediaObject>, ApplicationError> {
        Err(unavailable())
    }
    fn public_url(&self, _: &str) -> Result<String, ApplicationError> {
        Err(unavailable())
    }
}

fn decode_digest(value: &str) -> Result<[u8; 32], ApplicationError> {
    let mut output = [0_u8; 32];
    for (index, slot) in output.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
    }
    Ok(output)
}
fn malformed_object(field: &'static str) -> ApplicationError {
    ApplicationError::Unavailable {
        service: "media_storage",
        source: anyhow::anyhow!("stored Media object has invalid {field}"),
    }
}
fn unavailable() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "media_storage",
        source: anyhow::anyhow!("Media storage is not configured"),
    }
}
fn storage_error(error: impl std::error::Error + Send + Sync + 'static) -> ApplicationError {
    ApplicationError::Unavailable {
        service: "media_storage",
        source: anyhow::Error::new(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ports::MediaStorage;
    use axum::{Router, http::HeaderValue, routing::head};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chaos_domain::catalog::MediaDescriptor;
    use tokio::net::TcpListener;

    use super::{S3MediaStorage, S3MediaStorageConfiguration};

    #[tokio::test]
    async fn s3_adapter_signs_bound_puts_and_verifies_head_metadata() {
        let digest = [0xabu8; 32];
        let checksum = STANDARD.encode(digest);
        let application = Router::new().route(
            "/media-bucket/{*key}",
            head(move || {
                let checksum = Arc::new(checksum.clone());
                async move {
                    let mut response = axum::response::Response::new(axum::body::Body::empty());
                    let headers = response.headers_mut();
                    headers.insert("content-type", HeaderValue::from_static("image/webp"));
                    headers.insert("content-length", HeaderValue::from_static("1024"));
                    headers.insert(
                        "x-amz-checksum-sha256",
                        HeaderValue::from_str(checksum.as_str()).unwrap(),
                    );
                    response
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, application).await.unwrap() });
        let storage = S3MediaStorage::new(S3MediaStorageConfiguration {
            endpoint_url: Some(format!("http://{address}")),
            region: "us-east-1".into(),
            bucket: "media-bucket".into(),
            access_key_id: "test-access".into(),
            secret_access_key: "test-secret".into(),
            session_token: None,
            force_path_style: true,
            public_base_url: "https://assets.example.test/".parse().unwrap(),
        })
        .unwrap();
        let descriptor = MediaDescriptor::new(
            "front.webp",
            "image/webp",
            1024,
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "Front view",
        )
        .unwrap();
        let upload = storage
            .prepare_upload(
                "stores/019d0000-0000-7000-8000-000000000001/media/019d0000-0000-7000-8000-000000000002/original",
                &descriptor,
                std::time::Duration::from_secs(900),
                time::OffsetDateTime::now_utc() + time::Duration::minutes(15),
            )
            .await
            .unwrap();
        assert_eq!(upload.method, "PUT");
        assert!(upload.url.contains("X-Amz-Signature="));
        assert!(
            upload
                .headers
                .iter()
                .any(|(name, value)| name == "content-type" && value == "image/webp")
        );

        let stored = storage
            .inspect("stores/019d0000-0000-7000-8000-000000000001/media/019d0000-0000-7000-8000-000000000002/original")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.media_type, "image/webp");
        assert_eq!(stored.byte_size, 1024);
        assert_eq!(stored.sha256_hex, "ab".repeat(32));
        assert_eq!(
            storage.public_url("stores/a/media/b/original").unwrap(),
            "https://assets.example.test/stores/a/media/b/original"
        );
        server.abort();
    }
}

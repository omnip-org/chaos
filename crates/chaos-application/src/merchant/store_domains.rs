use std::sync::Arc;

use chaos_domain::merchant::{MerchantRole, SalesChannelId, StoreDomainId, StoreHostname, StoreId};
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    ports::{
        CreateStoreDomainRecord, IdempotencyRequest, ResolvedStoreDomain,
        StoreDomainCreationStatus, StoreDomainItem, StoreDomainOwnershipVerifier,
        StoreDomainRepository, StoreDomainVerificationMaterialGenerator,
    },
};

use super::{MerchantActor, Page};

pub struct CreateStoreDomainInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub hostname: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct CreateStoreDomainOutput {
    pub domain: StoreDomainItem,
    pub verification_record_name: String,
    pub verification_record_value: SecretString,
}

pub struct VerifyStoreDomainInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub domain_id: StoreDomainId,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct ArchiveStoreDomainInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub domain_id: StoreDomainId,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct StoreDomainAdministration {
    repository: Arc<dyn StoreDomainRepository>,
    material_generator: Arc<dyn StoreDomainVerificationMaterialGenerator>,
    verifier: Arc<dyn StoreDomainOwnershipVerifier>,
}

impl StoreDomainAdministration {
    pub fn new(
        repository: Arc<dyn StoreDomainRepository>,
        material_generator: Arc<dyn StoreDomainVerificationMaterialGenerator>,
        verifier: Arc<dyn StoreDomainOwnershipVerifier>,
    ) -> Self {
        Self {
            repository,
            material_generator,
            verifier,
        }
    }

    pub async fn create(
        &self,
        input: CreateStoreDomainInput,
    ) -> Result<CreateStoreDomainOutput, ApplicationError> {
        require_domain_administrator(input.actor)?;
        let hostname = StoreHostname::parse(input.hostname)?;
        let material = self.material_generator.generate();
        let result = self
            .repository
            .create(
                input.actor,
                CreateStoreDomainRecord {
                    store_id: input.store_id,
                    domain_id: StoreDomainId::new(),
                    sales_channel_id: input.sales_channel_id,
                    hostname: hostname.clone(),
                    verification_token_digest: material.token_digest,
                    created_at: input.now,
                },
                &input.idempotency,
            )
            .await?;
        if result.status == StoreDomainCreationStatus::Replayed {
            return Err(ApplicationError::Conflict {
                code: "store_domain_verification_token_already_issued",
                message: "the Store Domain was already created and its verification token cannot be shown again",
            });
        }
        Ok(CreateStoreDomainOutput {
            verification_record_name: hostname.verification_record_name(),
            verification_record_value: SecretString::from(format!(
                "chaos-domain-verification={}",
                secrecy::ExposeSecret::expose_secret(&material.plaintext_token)
            )),
            domain: result.item,
        })
    }

    pub async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<StoreDomainId>,
        limit: u16,
    ) -> Result<Page<StoreDomainItem>, ApplicationError> {
        require_domain_reader(actor)?;
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn verify(
        &self,
        input: VerifyStoreDomainInput,
    ) -> Result<StoreDomainItem, ApplicationError> {
        require_domain_administrator(input.actor)?;
        let pending = self
            .repository
            .pending_verification(input.actor, input.store_id, input.domain_id)
            .await?;
        if pending.verification_required
            && !self
                .verifier
                .verify(&pending.item.hostname, &pending.token_digest)
                .await?
        {
            return Err(ApplicationError::Conflict {
                code: "store_domain_verification_pending",
                message: "the expected DNS TXT verification record is not visible yet",
            });
        }
        self.repository
            .mark_verified(
                input.actor,
                input.store_id,
                input.domain_id,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn archive(
        &self,
        input: ArchiveStoreDomainInput,
    ) -> Result<StoreDomainItem, ApplicationError> {
        require_domain_administrator(input.actor)?;
        self.repository
            .archive(
                input.actor,
                input.store_id,
                input.domain_id,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn resolve(&self, hostname: String) -> Result<ResolvedStoreDomain, ApplicationError> {
        let hostname = StoreHostname::parse(hostname)?;
        self.repository
            .resolve(&hostname)
            .await?
            .ok_or(ApplicationError::NotFound {
                resource: "store_domain",
                id: hostname.as_str().into(),
            })
    }
}

fn require_domain_reader(actor: MerchantActor) -> Result<(), ApplicationError> {
    match actor.role() {
        MerchantRole::Owner
        | MerchantRole::Administrator
        | MerchantRole::Developer
        | MerchantRole::Manager
        | MerchantRole::Support => Ok(()),
    }
}

fn require_domain_administrator(actor: MerchantActor) -> Result<(), ApplicationError> {
    match actor.role() {
        MerchantRole::Owner | MerchantRole::Administrator => Ok(()),
        MerchantRole::Developer | MerchantRole::Manager | MerchantRole::Support => {
            Err(ApplicationError::Forbidden)
        }
    }
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

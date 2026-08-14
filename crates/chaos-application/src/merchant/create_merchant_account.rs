use std::sync::Arc;

use chaos_domain::{
    identity::UserId,
    merchant::{
        MerchantAccount, MerchantAccountId, MerchantAccountMembership, MerchantAccountSlug,
    },
};

use crate::{
    ApplicationError,
    ports::{IdempotencyRequest, MerchantProvisioningUnitOfWork},
};

pub struct CreateMerchantAccountInput {
    pub owner_user_id: UserId,
    pub slug: String,
    pub display_name: String,
    pub idempotency: IdempotencyRequest,
}

#[derive(Debug)]
pub struct CreateMerchantAccountOutput {
    pub merchant_account_id: MerchantAccountId,
}

pub struct CreateMerchantAccount {
    unit_of_work: Arc<dyn MerchantProvisioningUnitOfWork>,
}

impl CreateMerchantAccount {
    pub fn new(unit_of_work: Arc<dyn MerchantProvisioningUnitOfWork>) -> Self {
        Self { unit_of_work }
    }

    pub async fn execute(
        &self,
        input: CreateMerchantAccountInput,
    ) -> Result<CreateMerchantAccountOutput, ApplicationError> {
        let account =
            MerchantAccount::create(MerchantAccountSlug::parse(input.slug)?, input.display_name)?;
        let membership = MerchantAccountMembership::owner(account.id(), input.owner_user_id);
        let mut transaction = self
            .unit_of_work
            .begin(input.owner_user_id, account.id())
            .await?;
        if let Some(merchant_account_id) = transaction
            .reserve_account_creation(&input.idempotency)
            .await?
        {
            return Ok(CreateMerchantAccountOutput {
                merchant_account_id,
            });
        }
        transaction.insert_merchant_account(&account).await?;
        transaction.insert_membership(&membership).await?;
        transaction
            .complete_account_creation(&input.idempotency, account.id())
            .await?;
        transaction.commit().await?;

        Ok(CreateMerchantAccountOutput {
            merchant_account_id: account.id(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chaos_domain::merchant::MerchantRole;

    use super::*;
    use crate::ports::MerchantProvisioningTransaction;

    #[derive(Default)]
    struct RecordedWrites {
        account_ids: Vec<MerchantAccountId>,
        owner_roles: Vec<MerchantRole>,
        commits: usize,
    }

    struct FakeUnitOfWork {
        writes: Arc<Mutex<RecordedWrites>>,
    }

    struct FakeTransaction {
        writes: Arc<Mutex<RecordedWrites>>,
        pending_account_id: Option<MerchantAccountId>,
        pending_role: Option<MerchantRole>,
    }

    #[async_trait]
    impl MerchantProvisioningUnitOfWork for FakeUnitOfWork {
        async fn begin(
            &self,
            _owner_user_id: UserId,
            _merchant_account_id: MerchantAccountId,
        ) -> Result<Box<dyn MerchantProvisioningTransaction>, ApplicationError> {
            Ok(Box::new(FakeTransaction {
                writes: self.writes.clone(),
                pending_account_id: None,
                pending_role: None,
            }))
        }
    }

    #[async_trait]
    impl MerchantProvisioningTransaction for FakeTransaction {
        async fn reserve_account_creation(
            &mut self,
            _request: &IdempotencyRequest,
        ) -> Result<Option<MerchantAccountId>, ApplicationError> {
            Ok(None)
        }

        async fn insert_merchant_account(
            &mut self,
            account: &MerchantAccount,
        ) -> Result<(), ApplicationError> {
            self.pending_account_id = Some(account.id());
            Ok(())
        }

        async fn insert_membership(
            &mut self,
            membership: &MerchantAccountMembership,
        ) -> Result<(), ApplicationError> {
            self.pending_role = Some(membership.role());
            Ok(())
        }

        async fn complete_account_creation(
            &mut self,
            _request: &IdempotencyRequest,
            _merchant_account_id: MerchantAccountId,
        ) -> Result<(), ApplicationError> {
            Ok(())
        }

        async fn commit(self: Box<Self>) -> Result<(), ApplicationError> {
            let mut writes = self.writes.lock().unwrap();
            writes.account_ids.push(self.pending_account_id.unwrap());
            writes.owner_roles.push(self.pending_role.unwrap());
            writes.commits += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn creates_account_and_owner_membership_atomically() {
        let writes = Arc::new(Mutex::new(RecordedWrites::default()));
        let service = CreateMerchantAccount::new(Arc::new(FakeUnitOfWork {
            writes: writes.clone(),
        }));

        let output = service
            .execute(CreateMerchantAccountInput {
                owner_user_id: UserId::new(),
                slug: "acme".into(),
                display_name: "ACME".into(),
                idempotency: IdempotencyRequest {
                    key: "create-acme".into(),
                    request_fingerprint: [7; 32],
                },
            })
            .await
            .unwrap();

        let writes = writes.lock().unwrap();
        assert_eq!(writes.account_ids, vec![output.merchant_account_id]);
        assert_eq!(writes.owner_roles, vec![MerchantRole::Owner]);
        assert_eq!(writes.commits, 1);
    }
}

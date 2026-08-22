-- Stripe is the only payment processor in this deployment. Keep the store
-- account row as the operational boundary, but make the Stripe identifiers
-- explicit so Checkout Sessions and PaymentIntents are never conflated.

ALTER TABLE commerce.provider_accounts
    ADD CONSTRAINT provider_accounts_stripe_only_check
    CHECK (provider = 'stripe_checkout');

ALTER TABLE commerce.payment_attempts
    RENAME COLUMN provider_reference TO stripe_checkout_session_id;

ALTER TABLE commerce.payment_attempts
    ADD COLUMN stripe_payment_intent_id TEXT,
    ADD COLUMN stripe_charge_id TEXT;

ALTER TABLE commerce.refunds
    RENAME COLUMN provider_reference TO stripe_refund_id;

ALTER TABLE commerce.payment_attempts
    ADD CONSTRAINT payment_attempts_stripe_session_length_check CHECK (
        stripe_checkout_session_id IS NULL
        OR length(trim(stripe_checkout_session_id)) BETWEEN 1 AND 255
    ),
    ADD CONSTRAINT payment_attempts_stripe_payment_intent_check CHECK (
        stripe_payment_intent_id IS NULL
        OR stripe_payment_intent_id ~ '^pi_[A-Za-z0-9]+$'
    ),
    ADD CONSTRAINT payment_attempts_stripe_charge_check CHECK (
        stripe_charge_id IS NULL
        OR stripe_charge_id ~ '^ch_[A-Za-z0-9]+$'
    );

ALTER TABLE commerce.refunds
    ADD CONSTRAINT refunds_stripe_refund_length_check CHECK (
        stripe_refund_id IS NULL
        OR length(trim(stripe_refund_id)) BETWEEN 1 AND 255
    );

CREATE UNIQUE INDEX payment_attempts_stripe_payment_intent_key
    ON commerce.payment_attempts (stripe_payment_intent_id)
    WHERE stripe_payment_intent_id IS NOT NULL;

CREATE UNIQUE INDEX refunds_stripe_refund_key
    ON commerce.refunds (stripe_refund_id)
    WHERE stripe_refund_id IS NOT NULL;

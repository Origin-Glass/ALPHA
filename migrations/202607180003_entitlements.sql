CREATE TABLE organizations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,47}$'),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'archived')),
    created_by uuid NOT NULL REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE organization_memberships (
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role IN ('OWNER', 'ADMIN', 'INSTRUCTOR', 'MEMBER')),
    joined_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, user_id)
);

CREATE TABLE classes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    created_by uuid NOT NULL REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX classes_organization_idx ON classes (organization_id, created_at DESC);

CREATE TABLE class_memberships (
    class_id uuid NOT NULL REFERENCES classes(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role IN ('INSTRUCTOR', 'LEARNER')),
    joined_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (class_id, user_id)
);

CREATE TABLE plans (
    code text PRIMARY KEY CHECK (code ~ '^[a-z][a-z0-9_]{2,39}$'),
    name_ko text NOT NULL CHECK (char_length(name_ko) BETWEEN 1 AND 80),
    audience text NOT NULL CHECK (audience IN ('individual', 'education', 'event', 'assessment')),
    active boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO plans (code, name_ko, audience) VALUES
    ('free_individual', '무료 개인', 'individual'),
    ('premium_individual', '프리미엄 개인', 'individual'),
    ('education', '교육', 'education'),
    ('hosted_event', '호스팅 행사·대회', 'event'),
    ('recruitment_assessment', '채용·역량 평가', 'assessment');

CREATE TABLE plan_prices (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    plan_code text NOT NULL REFERENCES plans(code),
    currency char(3) NOT NULL DEFAULT 'KRW',
    amount_minor bigint NOT NULL CHECK (amount_minor >= 0),
    billing_interval text NOT NULL CHECK (billing_interval IN ('month', 'year', 'one_time')),
    active boolean NOT NULL DEFAULT true
);

CREATE TABLE plan_entitlements (
    plan_code text NOT NULL REFERENCES plans(code) ON DELETE CASCADE,
    feature_key text NOT NULL CHECK (feature_key ~ '^[a-z][a-z0-9_.]{2,79}$'),
    limit_value bigint CHECK (limit_value IS NULL OR limit_value >= 0),
    PRIMARY KEY (plan_code, feature_key)
);

INSERT INTO plan_entitlements (plan_code, feature_key, limit_value) VALUES
    ('free_individual', 'practice.core', NULL),
    ('free_individual', 'judge.standard.daily_submissions', 100),
    ('premium_individual', 'practice.core', NULL),
    ('premium_individual', 'judge.standard.daily_submissions', 300),
    ('premium_individual', 'analytics.advanced', NULL),
    ('education', 'practice.core', NULL),
    ('education', 'instructor.administration', NULL),
    ('hosted_event', 'contest.private.branded', NULL),
    ('recruitment_assessment', 'assessment.reports', NULL);

CREATE TABLE subscriptions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid REFERENCES users(id),
    organization_id uuid REFERENCES organizations(id),
    plan_code text NOT NULL REFERENCES plans(code),
    provider text NOT NULL,
    provider_subscription_id text,
    status text NOT NULL CHECK (status IN ('pending', 'active', 'past_due', 'cancelled', 'expired')),
    starts_at timestamptz,
    ends_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((user_id IS NOT NULL)::integer + (organization_id IS NOT NULL)::integer = 1),
    UNIQUE (provider, provider_subscription_id)
);

CREATE TABLE entitlement_grants (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid REFERENCES users(id),
    organization_id uuid REFERENCES organizations(id),
    feature_key text NOT NULL CHECK (feature_key ~ '^[a-z][a-z0-9_.]{2,79}$'),
    limit_value bigint CHECK (limit_value IS NULL OR limit_value >= 0),
    source_type text NOT NULL CHECK (source_type IN ('subscription', 'admin', 'migration')),
    source_id uuid,
    valid_from timestamptz NOT NULL DEFAULT now(),
    valid_until timestamptz,
    revoked_at timestamptz,
    CHECK ((user_id IS NOT NULL)::integer + (organization_id IS NOT NULL)::integer = 1),
    CHECK (valid_until IS NULL OR valid_until > valid_from)
);

CREATE INDEX entitlement_grants_active_user_idx
    ON entitlement_grants (user_id, feature_key, valid_until)
    WHERE revoked_at IS NULL;

CREATE TABLE checkout_intents (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id),
    plan_code text NOT NULL REFERENCES plans(code),
    provider text NOT NULL,
    idempotency_key text NOT NULL,
    status text NOT NULL CHECK (status IN ('created', 'redirected', 'completed', 'failed', 'expired')),
    provider_reference text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idempotency_key)
);

CREATE TABLE payment_provider_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    provider text NOT NULL,
    provider_event_id text NOT NULL,
    signature_verified boolean NOT NULL DEFAULT false,
    payload jsonb NOT NULL,
    processing_status text NOT NULL CHECK (processing_status IN ('received', 'applied', 'rejected', 'failed')),
    received_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_event_id)
);

CREATE TABLE refunds (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    subscription_id uuid NOT NULL REFERENCES subscriptions(id),
    provider_refund_id text,
    amount_minor bigint NOT NULL CHECK (amount_minor > 0),
    status text NOT NULL CHECK (status IN ('requested', 'completed', 'failed')),
    created_at timestamptz NOT NULL DEFAULT now()
);

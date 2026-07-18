ALTER TABLE roles DROP CONSTRAINT roles_role_check;
ALTER TABLE roles ADD CONSTRAINT roles_role_check CHECK (
    role IN (
        'USER', 'INSTRUCTOR', 'PROBLEM_SETTER', 'CONTEST_MANAGER', 'MODERATOR', 'ADMIN',
        'CONTENT_CREATOR', 'CONTENT_REVIEWER', 'RIGHTS_REVIEWER', 'AI_CONTENT_OPERATOR'
    )
);
INSERT INTO roles (role) VALUES ('AI_CONTENT_OPERATOR');

ALTER TABLE role_capabilities DROP CONSTRAINT role_capabilities_capability_check;
ALTER TABLE role_capabilities ADD CONSTRAINT role_capabilities_capability_check CHECK (
    capability IN (
        'content.create', 'content.review', 'rights.review', 'content.publish', 'governance.read',
        'content.generate', 'provider.configure', 'provider.use.frontier', 'provider.use.local',
        'provider.view_cost'
    )
);
INSERT INTO role_capabilities (role, capability) VALUES
    ('CONTENT_CREATOR', 'content.generate'),
    ('CONTENT_CREATOR', 'provider.use.frontier'),
    ('CONTENT_CREATOR', 'provider.use.local'),
    ('AI_CONTENT_OPERATOR', 'content.generate'),
    ('AI_CONTENT_OPERATOR', 'provider.use.frontier'),
    ('AI_CONTENT_OPERATOR', 'provider.use.local'),
    ('AI_CONTENT_OPERATOR', 'provider.view_cost'),
    ('ADMIN', 'content.generate'),
    ('ADMIN', 'provider.configure'),
    ('ADMIN', 'provider.use.frontier'),
    ('ADMIN', 'provider.use.local'),
    ('ADMIN', 'provider.view_cost');

CREATE TABLE content_provider_configs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9][a-z0-9_-]{2,39}$'),
    kind text NOT NULL CHECK (kind IN ('external', 'local')),
    protocol text NOT NULL CHECK (protocol IN ('openai_compatible', 'anthropic')),
    base_url text NOT NULL CHECK (char_length(base_url) BETWEEN 8 AND 500),
    model text NOT NULL CHECK (char_length(model) BETWEEN 1 AND 120),
    cost_per_generation_microunits bigint NOT NULL CHECK (cost_per_generation_microunits BETWEEN 1 AND 1000000000),
    credential_env_var text CHECK (credential_env_var IN (
        'CONTENT_AI_CUSTOM_API_KEY', 'OPENROUTER_API_KEY', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY'
    )),
    enabled boolean NOT NULL DEFAULT false,
    health_status text NOT NULL DEFAULT 'unverified' CHECK (health_status IN ('unverified', 'healthy', 'unhealthy')),
    last_health_at timestamptz,
    supports_stream boolean NOT NULL DEFAULT false,
    supports_tools boolean NOT NULL DEFAULT false,
    fallback_provider_id uuid REFERENCES content_provider_configs(id),
    created_by uuid NOT NULL REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz,
    CHECK ((kind = 'external') = (credential_env_var IS NOT NULL)),
    CHECK (
        (kind = 'local' AND protocol = 'openai_compatible' AND credential_env_var IS NULL)
        OR (kind = 'external' AND protocol = 'anthropic' AND credential_env_var = 'ANTHROPIC_API_KEY')
        OR (kind = 'external' AND protocol = 'openai_compatible' AND credential_env_var IN (
            'CONTENT_AI_CUSTOM_API_KEY', 'OPENROUTER_API_KEY', 'OPENAI_API_KEY'
        ))
    )
);

CREATE TABLE content_budgets (
    budget_key text PRIMARY KEY,
    limit_microunits bigint NOT NULL CHECK (limit_microunits >= 0),
    reserved_microunits bigint NOT NULL DEFAULT 0 CHECK (reserved_microunits >= 0),
    spent_microunits bigint NOT NULL DEFAULT 0 CHECK (spent_microunits >= 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (reserved_microunits + spent_microunits <= limit_microunits)
);
INSERT INTO content_budgets (budget_key, limit_microunits) VALUES ('global', 1000000);

CREATE TABLE content_generation_jobs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_by uuid NOT NULL REFERENCES users(id),
    provider_id uuid NOT NULL REFERENCES content_provider_configs(id),
    fallback_provider_id_snapshot uuid REFERENCES content_provider_configs(id),
    content_type text NOT NULL CHECK (content_type IN (
        'algorithm_problem', 'code_reading', 'debugging', 'documentation_lesson', 'implementation_task'
    )),
    request_spec jsonb NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    status text NOT NULL CHECK (status IN (
        'queued', 'leased', 'blocked_disabled', 'blocked_missing_credential',
        'completed', 'failed', 'cancelled'
    )),
    estimated_cost_microunits bigint NOT NULL CHECK (estimated_cost_microunits BETWEEN 1 AND 20000000000),
    attempt_cost_microunits bigint NOT NULL CHECK (attempt_cost_microunits BETWEEN 1 AND 20000000000),
    reserved_cost_microunits bigint NOT NULL DEFAULT 0 CHECK (reserved_cost_microunits >= 0),
    settled boolean NOT NULL DEFAULT false,
    attempt_count smallint NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts smallint NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 5),
    lease_owner text,
    lease_token uuid,
    lease_expires_at timestamptz,
    last_error_code text,
    available_at timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz,
    cloned_from_job_id uuid REFERENCES content_generation_jobs(id),
    clone_idempotency_key uuid,
    cancel_idempotency_key uuid,
    archive_idempotency_key uuid,
    CHECK ((status = 'leased') = (lease_owner IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK (reserved_cost_microunits IN (0, estimated_cost_microunits)),
    CHECK (estimated_cost_microunits = attempt_cost_microunits * max_attempts)
);
CREATE UNIQUE INDEX content_jobs_clone_idempotency_idx
    ON content_generation_jobs (created_by, cloned_from_job_id, clone_idempotency_key)
    WHERE clone_idempotency_key IS NOT NULL;
CREATE INDEX content_jobs_ready_idx ON content_generation_jobs (available_at, created_at)
    WHERE status = 'queued';
CREATE INDEX content_jobs_lease_idx ON content_generation_jobs (lease_expires_at)
    WHERE status = 'leased';

CREATE TABLE content_generation_attempts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id uuid NOT NULL REFERENCES content_generation_jobs(id),
    attempt_number smallint NOT NULL CHECK (attempt_number > 0),
    provider_snapshot jsonb NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    response_hash bytea CHECK (response_hash IS NULL OR octet_length(response_hash) = 32),
    status text NOT NULL CHECK (status IN ('running', 'retryable_failure', 'failed', 'completed', 'cancelled')),
    error_code text,
    started_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    UNIQUE (job_id, attempt_number)
);

CREATE TABLE content_artifacts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id uuid NOT NULL REFERENCES content_generation_jobs(id),
    attempt_id uuid NOT NULL REFERENCES content_generation_attempts(id),
    artifact_kind text NOT NULL CHECK (artifact_kind IN ('candidate', 'provider_response', 'error_receipt')),
    payload jsonb NOT NULL,
    content_hash bytea NOT NULL CHECK (octet_length(content_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (attempt_id, artifact_kind, content_hash)
);

CREATE FUNCTION reject_content_artifact_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'content artifacts are append-only';
END;
$$;
CREATE TRIGGER content_artifacts_immutable
BEFORE UPDATE OR DELETE ON content_artifacts
FOR EACH ROW EXECUTE FUNCTION reject_content_artifact_mutation();

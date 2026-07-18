ALTER TABLE roles DROP CONSTRAINT roles_role_check;
ALTER TABLE roles ADD CONSTRAINT roles_role_check CHECK (role IN (
    'USER','INSTRUCTOR','PROBLEM_SETTER','CONTEST_MANAGER','MODERATOR','ADMIN',
    'CONTENT_CREATOR','CONTENT_REVIEWER','RIGHTS_REVIEWER','AI_CONTENT_OPERATOR',
    'HUMAN_REVIEWER','LEGAL_REVIEWER'
));
INSERT INTO roles (role) VALUES ('HUMAN_REVIEWER'), ('LEGAL_REVIEWER');

ALTER TABLE role_capabilities DROP CONSTRAINT role_capabilities_capability_check;
ALTER TABLE role_capabilities ADD CONSTRAINT role_capabilities_capability_check CHECK (capability IN (
    'content.create','content.review','rights.review','content.publish','governance.read',
    'content.generate','provider.configure','provider.use.frontier','provider.use.local','provider.view_cost',
    'content.review.ai','content.review.human','content.review.legal','content.retire'
));
INSERT INTO role_capabilities (role, capability) VALUES
    ('AI_CONTENT_OPERATOR','content.review.ai'),
    ('HUMAN_REVIEWER','content.review.human'), ('HUMAN_REVIEWER','governance.read'),
    ('LEGAL_REVIEWER','content.review.legal'), ('LEGAL_REVIEWER','governance.read'),
    ('CONTENT_REVIEWER','content.review.human'),
    ('RIGHTS_REVIEWER','content.review.legal'),
    ('ADMIN','content.review.ai'), ('ADMIN','content.review.human'),
    ('ADMIN','content.review.legal'), ('ADMIN','content.retire');

CREATE TABLE content_review_items (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact_id uuid NOT NULL REFERENCES content_artifacts(id),
    artifact_hash bytea NOT NULL CHECK (octet_length(artifact_hash) = 32),
    revision_number integer NOT NULL DEFAULT 1 CHECK (revision_number > 0),
    author_user_id uuid NOT NULL REFERENCES users(id),
    state text NOT NULL DEFAULT 'ai_review_pending' CHECK (state IN (
        'ai_review_pending','changes_requested','human_review_pending','rights_review_pending',
        'pilot_pending','approved','published','unpublished','removal_pending','rejected'
    )),
    impact text NOT NULL CHECK (impact IN ('standard','high','novel')),
    human_reviewer_id uuid REFERENCES users(id),
    rights_reviewer_id uuid REFERENCES users(id),
    published_by uuid REFERENCES users(id),
    create_idempotency_key uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    published_at timestamptz,
    UNIQUE (author_user_id, create_idempotency_key),
    CHECK (human_reviewer_id IS NULL OR human_reviewer_id <> author_user_id),
    CHECK (rights_reviewer_id IS NULL OR rights_reviewer_id <> author_user_id),
    CHECK (human_reviewer_id IS NULL OR rights_reviewer_id IS NULL OR human_reviewer_id <> rights_reviewer_id),
    CHECK (published_by IS NULL OR (published_by <> author_user_id AND published_by IS DISTINCT FROM human_reviewer_id AND published_by IS DISTINCT FROM rights_reviewer_id))
);

CREATE TABLE content_ai_review_receipts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    review_item_id uuid NOT NULL REFERENCES content_review_items(id),
    revision_number integer NOT NULL CHECK (revision_number > 0),
    review_kind text NOT NULL CHECK (review_kind IN ('specification_pedagogy','solution_judge','adversarial_rights')),
    operator_user_id uuid NOT NULL REFERENCES users(id),
    provider text NOT NULL CHECK (char_length(provider) BETWEEN 1 AND 120),
    model text NOT NULL CHECK (char_length(model) BETWEEN 1 AND 120),
    prompt_version text NOT NULL CHECK (char_length(prompt_version) BETWEEN 3 AND 120),
    role_identifier text NOT NULL CHECK (char_length(role_identifier) BETWEEN 3 AND 120),
    prompt_hash bytea NOT NULL CHECK (octet_length(prompt_hash) = 32),
    review_seed bigint NOT NULL,
    input_hash bytea NOT NULL CHECK (octet_length(input_hash) = 32),
    output_hash bytea NOT NULL CHECK (octet_length(output_hash) = 32),
    latency_ms integer NOT NULL CHECK (latency_ms BETWEEN 1 AND 3600000),
    usage jsonb NOT NULL CHECK (jsonb_typeof(usage) = 'object'),
    outcome text NOT NULL CHECK (outcome IN ('pass','fail')),
    findings jsonb NOT NULL CHECK (jsonb_typeof(findings) = 'array'),
    idempotency_key uuid NOT NULL,
    receipt_hash bytea NOT NULL CHECK (octet_length(receipt_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (review_item_id, revision_number, review_kind),
    UNIQUE (review_item_id, revision_number, output_hash),
    UNIQUE (review_item_id, revision_number, provider, model, prompt_hash, review_seed),
    UNIQUE (review_item_id, operator_user_id, idempotency_key)
);

CREATE TABLE content_human_review_decisions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    review_item_id uuid NOT NULL REFERENCES content_review_items(id),
    revision_number integer NOT NULL,
    reviewer_user_id uuid NOT NULL REFERENCES users(id),
    decision text NOT NULL CHECK (decision IN ('approve','changes_requested','reject')),
    note text NOT NULL CHECK (char_length(note) BETWEEN 3 AND 2000),
    idempotency_key uuid NOT NULL,
    receipt_hash bytea NOT NULL CHECK (octet_length(receipt_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (review_item_id, reviewer_user_id, idempotency_key)
);

CREATE TABLE content_rights_review_receipts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    review_item_id uuid NOT NULL REFERENCES content_review_items(id),
    revision_number integer NOT NULL,
    reviewer_user_id uuid NOT NULL REFERENCES users(id),
    decision text NOT NULL CHECK (decision IN ('approve','reject')),
    basis text NOT NULL CHECK (basis IN ('original','licensed','public_domain','contract')),
    evidence text NOT NULL CHECK (char_length(evidence) BETWEEN 3 AND 4000),
    commercial_use_allowed boolean NOT NULL,
    redistribution_allowed boolean NOT NULL,
    provider_terms_version text NOT NULL CHECK (char_length(provider_terms_version) BETWEEN 3 AND 120),
    idempotency_key uuid NOT NULL,
    receipt_hash bytea NOT NULL CHECK (octet_length(receipt_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (review_item_id, reviewer_user_id, idempotency_key)
);

CREATE TABLE content_pilot_receipts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    review_item_id uuid NOT NULL REFERENCES content_review_items(id),
    revision_number integer NOT NULL,
    reviewer_user_id uuid NOT NULL REFERENCES users(id),
    cohort text NOT NULL CHECK (char_length(cohort) BETWEEN 3 AND 200),
    started_at timestamptz NOT NULL,
    ended_at timestamptz NOT NULL CHECK (ended_at > started_at),
    participants integer NOT NULL CHECK (participants >= 5),
    completion_rate double precision NOT NULL CHECK (completion_rate BETWEEN 0 AND 1),
    failure_rate double precision NOT NULL CHECK (failure_rate BETWEEN 0 AND 1),
    report_count integer NOT NULL CHECK (report_count >= 0),
    rollback_ready boolean NOT NULL,
    decision text NOT NULL CHECK (decision IN ('pass','fail')),
    note text NOT NULL CHECK (char_length(note) BETWEEN 3 AND 2000),
    idempotency_key uuid NOT NULL,
    receipt_hash bytea NOT NULL CHECK (octet_length(receipt_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (review_item_id, reviewer_user_id, idempotency_key)
);

CREATE TABLE content_review_revisions (
    review_item_id uuid NOT NULL REFERENCES content_review_items(id),
    revision_number integer NOT NULL,
    artifact_id uuid NOT NULL REFERENCES content_artifacts(id),
    artifact_hash bytea NOT NULL CHECK (octet_length(artifact_hash) = 32),
    changed_by uuid NOT NULL REFERENCES users(id),
    idempotency_key uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (review_item_id, revision_number),
    UNIQUE (review_item_id, revision_number, artifact_hash),
    UNIQUE (review_item_id, changed_by, idempotency_key)
);

ALTER TABLE content_ai_review_receipts ADD CONSTRAINT content_ai_receipt_exact_revision_fk
    FOREIGN KEY (review_item_id, revision_number, input_hash)
    REFERENCES content_review_revisions (review_item_id, revision_number, artifact_hash) ON DELETE RESTRICT;
ALTER TABLE content_human_review_decisions ADD CONSTRAINT content_human_decision_revision_fk
    FOREIGN KEY (review_item_id, revision_number)
    REFERENCES content_review_revisions (review_item_id, revision_number) ON DELETE RESTRICT;
ALTER TABLE content_rights_review_receipts ADD CONSTRAINT content_rights_receipt_revision_fk
    FOREIGN KEY (review_item_id, revision_number)
    REFERENCES content_review_revisions (review_item_id, revision_number) ON DELETE RESTRICT;
ALTER TABLE content_pilot_receipts ADD CONSTRAINT content_pilot_receipt_revision_fk
    FOREIGN KEY (review_item_id, revision_number)
    REFERENCES content_review_revisions (review_item_id, revision_number) ON DELETE RESTRICT;

CREATE INDEX content_reviews_queue_idx ON content_review_items (state, updated_at DESC);
CREATE INDEX content_reviews_public_idx ON content_review_items (published_at DESC) WHERE state = 'published';

CREATE FUNCTION reject_content_review_evidence_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'content review evidence is append-only'; END;
$$;
CREATE TRIGGER content_ai_receipts_immutable BEFORE UPDATE OR DELETE ON content_ai_review_receipts FOR EACH ROW EXECUTE FUNCTION reject_content_review_evidence_mutation();
CREATE TRIGGER content_human_decisions_immutable BEFORE UPDATE OR DELETE ON content_human_review_decisions FOR EACH ROW EXECUTE FUNCTION reject_content_review_evidence_mutation();
CREATE TRIGGER content_rights_receipts_immutable BEFORE UPDATE OR DELETE ON content_rights_review_receipts FOR EACH ROW EXECUTE FUNCTION reject_content_review_evidence_mutation();
CREATE TRIGGER content_pilot_receipts_immutable BEFORE UPDATE OR DELETE ON content_pilot_receipts FOR EACH ROW EXECUTE FUNCTION reject_content_review_evidence_mutation();
CREATE TRIGGER content_review_revisions_immutable BEFORE UPDATE OR DELETE ON content_review_revisions FOR EACH ROW EXECUTE FUNCTION reject_content_review_evidence_mutation();

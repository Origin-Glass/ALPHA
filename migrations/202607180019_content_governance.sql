ALTER TABLE roles DROP CONSTRAINT roles_role_check;
ALTER TABLE roles ADD CONSTRAINT roles_role_check CHECK (
    role IN (
        'USER', 'INSTRUCTOR', 'PROBLEM_SETTER', 'CONTEST_MANAGER', 'MODERATOR', 'ADMIN',
        'CONTENT_CREATOR', 'CONTENT_REVIEWER', 'RIGHTS_REVIEWER'
    )
);

INSERT INTO roles (role) VALUES
    ('CONTENT_CREATOR'), ('CONTENT_REVIEWER'), ('RIGHTS_REVIEWER');

CREATE TABLE role_capabilities (
    role text NOT NULL REFERENCES roles(role) ON DELETE CASCADE,
    capability text NOT NULL CHECK (capability IN (
        'content.create', 'content.review', 'rights.review', 'content.publish', 'governance.read'
    )),
    PRIMARY KEY (role, capability)
);

INSERT INTO role_capabilities (role, capability) VALUES
    ('CONTENT_CREATOR', 'content.create'),
    ('CONTENT_CREATOR', 'content.publish'),
    ('CONTENT_CREATOR', 'governance.read'),
    ('CONTENT_REVIEWER', 'content.review'),
    ('CONTENT_REVIEWER', 'governance.read'),
    ('RIGHTS_REVIEWER', 'rights.review'),
    ('RIGHTS_REVIEWER', 'governance.read'),
    ('PROBLEM_SETTER', 'content.create'),
    ('ADMIN', 'content.create'),
    ('ADMIN', 'content.review'),
    ('ADMIN', 'rights.review'),
    ('ADMIN', 'content.publish'),
    ('ADMIN', 'governance.read');

CREATE TABLE content_governance (
    problem_revision_id uuid PRIMARY KEY REFERENCES problem_revisions(id) ON DELETE CASCADE,
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE CASCADE,
    author_user_id uuid NOT NULL REFERENCES users(id),
    rights_basis text NOT NULL CHECK (rights_basis IN ('original', 'licensed', 'public_domain')),
    rights_evidence text NOT NULL CHECK (char_length(rights_evidence) BETWEEN 3 AND 2000),
    commercial_use_allowed boolean NOT NULL,
    redistribution_allowed boolean NOT NULL,
    content_reviewed_by uuid REFERENCES users(id),
    content_review_note text,
    content_reviewed_at timestamptz,
    rights_reviewed_by uuid REFERENCES users(id),
    rights_review_note text,
    rights_reviewed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (problem_revision_id, problem_id),
    CHECK (content_reviewed_by IS NULL OR content_reviewed_by <> author_user_id),
    CHECK (rights_reviewed_by IS NULL OR rights_reviewed_by <> author_user_id),
    CHECK (content_reviewed_by IS NULL OR rights_reviewed_by IS NULL OR content_reviewed_by <> rights_reviewed_by),
    CHECK ((content_reviewed_by IS NULL) = (content_reviewed_at IS NULL)),
    CHECK ((rights_reviewed_by IS NULL) = (rights_reviewed_at IS NULL))
);

CREATE INDEX content_governance_problem_idx ON content_governance (problem_id, updated_at DESC);

CREATE TABLE policy_versions (
    version text PRIMARY KEY,
    title_ko text NOT NULL,
    required boolean NOT NULL DEFAULT true,
    published_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO policy_versions (version, title_ko) VALUES
    ('2026-07-18', '이용약관 및 개인정보 처리방침');

CREATE TABLE policy_consents (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    policy_version text NOT NULL REFERENCES policy_versions(version),
    consented_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, policy_version)
);

INSERT INTO policy_consents (user_id, policy_version, consented_at)
SELECT id, terms_accepted_version, terms_accepted_at
FROM users
WHERE terms_accepted_version = '2026-07-18';

CREATE TABLE data_requests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('export', 'delete')),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processing', 'completed', 'cancelled')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX data_requests_user_time_idx ON data_requests (user_id, created_at DESC);

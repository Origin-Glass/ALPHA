CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    handle text NOT NULL UNIQUE CHECK (handle ~ '^[a-z0-9][a-z0-9-]{2,31}$'),
    display_name text NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 80),
    email text,
    email_verified boolean NOT NULL DEFAULT false,
    terms_accepted_at timestamptz,
    terms_accepted_version text,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'deleted')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((terms_accepted_at IS NULL) = (terms_accepted_version IS NULL))
);

CREATE UNIQUE INDEX users_normalized_email_idx
    ON users (lower(email))
    WHERE email IS NOT NULL AND status <> 'deleted';

CREATE TABLE roles (
    role text PRIMARY KEY CHECK (
        role IN ('USER', 'INSTRUCTOR', 'PROBLEM_SETTER', 'CONTEST_MANAGER', 'MODERATOR', 'ADMIN')
    )
);

INSERT INTO roles (role) VALUES
    ('USER'), ('INSTRUCTOR'), ('PROBLEM_SETTER'), ('CONTEST_MANAGER'), ('MODERATOR'), ('ADMIN');

CREATE TABLE user_roles (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role text NOT NULL REFERENCES roles(role),
    granted_by uuid REFERENCES users(id),
    granted_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, role)
);

CREATE TABLE oauth_identities (
    provider text NOT NULL CHECK (provider IN ('google', 'github', 'test')),
    subject text NOT NULL CHECK (char_length(subject) BETWEEN 1 AND 255),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    email_at_link text,
    email_verified_at_link boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_login_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, subject)
);

CREATE TABLE oauth_transactions (
    state_hash bytea PRIMARY KEY,
    provider text NOT NULL CHECK (provider IN ('google', 'github')),
    pkce_verifier text NOT NULL CHECK (char_length(pkce_verifier) BETWEEN 43 AND 128),
    redirect_after text NOT NULL DEFAULT '/',
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (redirect_after LIKE '/%' AND redirect_after NOT LIKE '//%')
);

CREATE INDEX oauth_transactions_expiry_idx ON oauth_transactions (expires_at);

CREATE TABLE sessions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_token_hash bytea NOT NULL UNIQUE,
    csrf_token_hash bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);

CREATE INDEX sessions_active_user_idx
    ON sessions (user_id, expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE audit_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_user_id uuid REFERENCES users(id),
    action text NOT NULL CHECK (char_length(action) BETWEEN 1 AND 120),
    target_type text NOT NULL CHECK (char_length(target_type) BETWEEN 1 AND 80),
    target_id text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_actor_time_idx ON audit_events (actor_user_id, occurred_at DESC);

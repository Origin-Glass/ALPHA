CREATE TABLE learning_plan_revisions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    revision integer NOT NULL CHECK (revision > 0),
    rule_version text NOT NULL CHECK (rule_version = 'project-learning-v1'),
    idempotency_key uuid NOT NULL,
    input jsonb NOT NULL CHECK (jsonb_typeof(input) = 'object'),
    input_hash bytea NOT NULL CHECK (octet_length(input_hash) = 32),
    plan_hash bytea NOT NULL CHECK (octet_length(plan_hash) = 32),
    target_outcome text NOT NULL CHECK (char_length(target_outcome) BETWEEN 1 AND 300),
    weekly_minutes integer NOT NULL CHECK (weekly_minutes BETWEEN 30 AND 2400),
    preferred_language text NOT NULL CHECK (preferred_language ~ '^[a-z0-9+#.-]{1,32}$'),
    path_mode text NOT NULL CHECK (path_mode IN ('structured', 'exploratory')),
    recommendation_key text NOT NULL CHECK (recommendation_key ~ '^[a-z0-9-]{3,64}$'),
    reason_codes jsonb NOT NULL CHECK (jsonb_typeof(reason_codes) = 'array'),
    items jsonb NOT NULL CHECK (jsonb_typeof(items) = 'array' AND jsonb_array_length(items) BETWEEN 1 AND 12),
    provider_used boolean NOT NULL DEFAULT false CHECK (provider_used = false),
    supersedes_id uuid REFERENCES learning_plan_revisions(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, revision),
    UNIQUE (user_id, idempotency_key)
);

CREATE TABLE learning_recommendation_rejections (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    recommendation_key text NOT NULL CHECK (recommendation_key ~ '^[a-z0-9-]{3,64}$'),
    reason text NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 500),
    idempotency_key uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, recommendation_key),
    UNIQUE (user_id, idempotency_key)
);

CREATE TABLE project_ideas (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    revision integer NOT NULL DEFAULT 1 CHECK (revision > 0),
    idempotency_key uuid NOT NULL,
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    motivation text NOT NULL CHECK (char_length(motivation) BETWEEN 1 AND 500),
    target_user text NOT NULL CHECK (char_length(target_user) BETWEEN 1 AND 200),
    intended_outcome text NOT NULL CHECK (char_length(intended_outcome) BETWEEN 1 AND 300),
    core_feature text NOT NULL CHECK (char_length(core_feature) BETWEEN 1 AND 120),
    technology text NOT NULL CHECK (technology ~ '^[a-z0-9+#.-]{1,32}$'),
    weekly_minutes integer NOT NULL CHECK (weekly_minutes BETWEEN 30 AND 2400),
    assistance_policy text NOT NULL CHECK (assistance_policy IN ('guided_ai','socratic_ai','documentation_navigator','curated_documentation','cheat_sheet_only','independent','transfer_challenge')),
    requested_features jsonb NOT NULL CHECK (jsonb_typeof(requested_features) = 'array' AND jsonb_array_length(requested_features) BETWEEN 1 AND 20),
    scoped_features jsonb NOT NULL CHECK (jsonb_typeof(scoped_features) = 'array' AND jsonb_array_length(scoped_features) BETWEEN 1 AND 5),
    milestones jsonb NOT NULL CHECK (jsonb_typeof(milestones) = 'array' AND jsonb_array_length(milestones) BETWEEN 1 AND 3),
    scope_reduced boolean NOT NULL,
    provider_used boolean NOT NULL DEFAULT false CHECK (provider_used = false),
    rule_version text NOT NULL CHECK (rule_version = 'project-learning-v1'),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idempotency_key)
);

CREATE TABLE learner_projects (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idea_id uuid NOT NULL REFERENCES project_ideas(id) ON DELETE RESTRICT,
    idempotency_key uuid NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','completed','archived')),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idea_id),
    UNIQUE (user_id, idempotency_key)
);

CREATE TABLE project_milestones (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES learner_projects(id) ON DELETE CASCADE,
    position smallint NOT NULL CHECK (position BETWEEN 1 AND 3),
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 160),
    estimated_minutes integer NOT NULL CHECK (estimated_minutes BETWEEN 30 AND 120),
    visible_result boolean NOT NULL,
    status text NOT NULL DEFAULT 'planned' CHECK (status IN ('planned','active','completed')),
    UNIQUE (project_id, position),
    CHECK (position <> 1 OR visible_result)
);

CREATE TABLE project_assistance_states (
    project_id uuid NOT NULL REFERENCES learner_projects(id) ON DELETE CASCADE,
    milestone_id uuid NOT NULL REFERENCES project_milestones(id) ON DELETE CASCADE,
    skill text NOT NULL CHECK (skill ~ '^[a-z0-9+#.-]{1,32}$'),
    level smallint NOT NULL CHECK (level BETWEEN 1 AND 7),
    consecutive_failures smallint NOT NULL DEFAULT 0 CHECK (consecutive_failures BETWEEN 0 AND 100),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, milestone_id, skill)
);

CREATE TABLE project_learning_events (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id uuid REFERENCES learner_projects(id) ON DELETE CASCADE,
    milestone_id uuid REFERENCES project_milestones(id) ON DELETE CASCADE,
    event_kind text NOT NULL CHECK (event_kind IN ('project_created','assistance_evidence','assistance_requested')),
    skill text CHECK (skill IS NULL OR skill ~ '^[a-z0-9+#.-]{1,32}$'),
    successful boolean,
    previous_level smallint CHECK (previous_level BETWEEN 1 AND 7),
    resulting_level smallint CHECK (resulting_level BETWEEN 1 AND 7),
    idempotency_key uuid,
    metadata jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idempotency_key)
);


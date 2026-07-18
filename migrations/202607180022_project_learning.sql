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
    restored_from_id uuid REFERENCES learning_plan_revisions(id) ON DELETE RESTRICT,
    deadline date NOT NULL,
    preferred_framework text NOT NULL CHECK (char_length(preferred_framework) BETWEEN 1 AND 64),
    desired_project text NOT NULL CHECK (char_length(desired_project) BETWEEN 1 AND 200),
    required_curriculum jsonb NOT NULL CHECK (jsonb_typeof(required_curriculum)='array' AND jsonb_array_length(required_curriculum) BETWEEN 1 AND 12),
    instructor_constraints jsonb NOT NULL CHECK (jsonb_typeof(instructor_constraints)='array' AND jsonb_array_length(instructor_constraints)<=10),
    assessment_checkpoints jsonb NOT NULL CHECK (jsonb_typeof(assessment_checkpoints)='array' AND jsonb_array_length(assessment_checkpoints) BETWEEN 1 AND 12),
    assistance_policy text NOT NULL CHECK (assistance_policy IN ('guided_ai','socratic_ai','documentation_navigator','curated_documentation','cheat_sheet_only','independent','transfer_challenge')),
    privacy text NOT NULL CHECK (privacy IN ('private','instructors','classroom')),
    origin text NOT NULL CHECK (origin IN ('learner','template_assignment')),
    template_id uuid,
    locked_requirements jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(locked_requirements)='array' AND jsonb_array_length(locked_requirements)<=12),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, revision),
    UNIQUE (user_id, idempotency_key),
    CHECK (
        (origin='learner' AND template_id IS NULL AND jsonb_array_length(locked_requirements)=0)
        OR
        (origin='template_assignment' AND template_id IS NOT NULL AND jsonb_array_length(locked_requirements)>0)
    )
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
    skill_level text NOT NULL CHECK (skill_level IN ('beginner','intermediate','advanced')),
    runtime text NOT NULL CHECK (runtime IN ('browser','cli','server')),
    infrastructure text NOT NULL CHECK (infrastructure IN ('local_only','container_available')),
    requested_features jsonb NOT NULL CHECK (jsonb_typeof(requested_features) = 'array' AND jsonb_array_length(requested_features) BETWEEN 1 AND 20),
    scoped_features jsonb NOT NULL CHECK (jsonb_typeof(scoped_features) = 'array' AND jsonb_array_length(scoped_features) BETWEEN 1 AND 5),
    milestones jsonb NOT NULL CHECK (jsonb_typeof(milestones) = 'array' AND jsonb_array_length(milestones) BETWEEN 1 AND 3),
    feasibility_reasons jsonb NOT NULL CHECK (jsonb_typeof(feasibility_reasons) = 'array' AND jsonb_array_length(feasibility_reasons) > 0),
    excluded_features jsonb NOT NULL CHECK (jsonb_typeof(excluded_features) = 'array'),
    scope_reduced boolean NOT NULL,
    provider_used boolean NOT NULL DEFAULT false CHECK (provider_used = false),
    rule_version text NOT NULL CHECK (rule_version = 'project-learning-v1'),
    source_kind text NOT NULL DEFAULT 'original' CHECK (source_kind IN ('original','imported')),
    repository_url text CHECK (repository_url IS NULL OR (char_length(repository_url) BETWEEN 10 AND 500 AND repository_url ~ '^https://[^/?#]+(/[^?#]*)?$' AND repository_url !~ '@')),
    repository_revision text CHECK (repository_revision IS NULL OR repository_revision ~ '^([0-9a-f]{40}|[0-9a-f]{64})$'),
    ownership_basis text CHECK (ownership_basis IS NULL OR ownership_basis IN ('learner_owned','authorized_import')),
    license_identifier text CHECK (license_identifier IS NULL OR license_identifier ~ '^[A-Za-z0-9.-]{2,40}$'),
    lineage_id uuid NOT NULL,
    supersedes_id uuid,
    restored_from_id uuid,
    input jsonb NOT NULL CHECK (jsonb_typeof(input)='object'),
    input_hash bytea NOT NULL CHECK (octet_length(input_hash)=32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idempotency_key),
    UNIQUE (id, user_id),
    UNIQUE (lineage_id, revision),
    FOREIGN KEY (supersedes_id, user_id) REFERENCES project_ideas(id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (restored_from_id, user_id) REFERENCES project_ideas(id, user_id) ON DELETE RESTRICT,
    CHECK (
        (source_kind = 'original' AND repository_url IS NULL AND repository_revision IS NULL AND ownership_basis IS NULL AND license_identifier IS NULL)
        OR
        (source_kind = 'imported' AND repository_url IS NOT NULL AND repository_revision IS NOT NULL AND ownership_basis IS NOT NULL AND license_identifier IS NOT NULL)
    )
);

CREATE TABLE learner_projects (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idea_id uuid NOT NULL,
    idempotency_key uuid NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','completed','archived')),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idea_id),
    UNIQUE (user_id, idempotency_key),
    UNIQUE (id, user_id),
    FOREIGN KEY (idea_id, user_id) REFERENCES project_ideas(id, user_id) ON DELETE RESTRICT
);

CREATE TABLE project_idea_confirmations (
    id uuid PRIMARY KEY,
    idea_id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scoped_features jsonb NOT NULL CHECK (jsonb_typeof(scoped_features)='array' AND jsonb_array_length(scoped_features) BETWEEN 1 AND 5),
    milestones jsonb NOT NULL CHECK (jsonb_typeof(milestones)='array' AND jsonb_array_length(milestones) BETWEEN 1 AND 3),
    confirmation_hash bytea NOT NULL CHECK (octet_length(confirmation_hash)=32),
    idempotency_key uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (idea_id, user_id),
    UNIQUE (user_id, idempotency_key),
    FOREIGN KEY (idea_id, user_id) REFERENCES project_ideas(id, user_id) ON DELETE RESTRICT
);

CREATE TABLE project_milestones (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    position smallint NOT NULL CHECK (position BETWEEN 1 AND 3),
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 160),
    estimated_minutes integer NOT NULL CHECK (estimated_minutes BETWEEN 30 AND 120),
    visible_result boolean NOT NULL,
    required_activity_id uuid NOT NULL REFERENCES learning_activities(id) ON DELETE RESTRICT,
    status text NOT NULL DEFAULT 'planned' CHECK (status IN ('planned','active','completed')),
    UNIQUE (project_id, position),
    UNIQUE (id, user_id),
    CHECK (position <> 1 OR visible_result),
    FOREIGN KEY (project_id, user_id) REFERENCES learner_projects(id, user_id) ON DELETE CASCADE
);

CREATE TABLE project_assistance_states (
    project_id uuid NOT NULL,
    milestone_id uuid NOT NULL,
    user_id uuid NOT NULL,
    skill text NOT NULL CHECK (skill ~ '^[a-z0-9+#.-]{1,32}$'),
    level smallint NOT NULL CHECK (level BETWEEN 1 AND 7),
    mode text NOT NULL CHECK (mode IN ('guided_ai','socratic_ai','documentation_navigator','curated_documentation','cheat_sheet_only','independent','transfer_challenge')),
    consecutive_failures smallint NOT NULL DEFAULT 0 CHECK (consecutive_failures BETWEEN 0 AND 100),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, milestone_id, user_id, skill),
    FOREIGN KEY (project_id, user_id) REFERENCES learner_projects(id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (milestone_id, user_id) REFERENCES project_milestones(id, user_id) ON DELETE CASCADE
);

CREATE TABLE project_learning_events (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id uuid,
    milestone_id uuid,
    event_kind text NOT NULL CHECK (event_kind IN ('project_created','assistance_evidence','assistance_requested')),
    skill text CHECK (skill IS NULL OR skill ~ '^[a-z0-9+#.-]{1,32}$'),
    successful boolean,
    previous_level smallint CHECK (previous_level BETWEEN 1 AND 7),
    resulting_level smallint CHECK (resulting_level BETWEEN 1 AND 7),
    idempotency_key uuid,
    source_attempt_id uuid REFERENCES activity_attempts(id) ON DELETE RESTRICT,
    metadata jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, idempotency_key),
    FOREIGN KEY (project_id, user_id) REFERENCES learner_projects(id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (milestone_id, user_id) REFERENCES project_milestones(id, user_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX project_learning_events_mastery_attempt_once
ON project_learning_events (source_attempt_id)
WHERE source_attempt_id IS NOT NULL;

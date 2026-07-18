ALTER TABLE problems
    ADD COLUMN checker_kind text NOT NULL DEFAULT 'whitespace'
        CHECK (checker_kind IN ('exact', 'whitespace', 'float')),
    ADD COLUMN float_tolerance double precision
        CHECK (float_tolerance IS NULL OR (float_tolerance > 0 AND float_tolerance <= 0.1));

CREATE TABLE problem_revisions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE CASCADE,
    version integer NOT NULL CHECK (version > 0),
    statement_ko text NOT NULL CHECK (char_length(statement_ko) BETWEEN 1 AND 50000),
    time_limit_ms integer NOT NULL CHECK (time_limit_ms BETWEEN 100 AND 30000),
    memory_limit_mb integer NOT NULL CHECK (memory_limit_mb BETWEEN 16 AND 2048),
    checker_kind text NOT NULL CHECK (checker_kind IN ('exact', 'whitespace', 'float')),
    float_tolerance double precision,
    content_hash bytea NOT NULL CHECK (octet_length(content_hash) = 32),
    authored_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (problem_id, version),
    CHECK ((checker_kind = 'float') = (float_tolerance IS NOT NULL))
);

ALTER TABLE problems ADD COLUMN current_revision_id uuid REFERENCES problem_revisions(id);

INSERT INTO problem_revisions (
    problem_id, version, statement_ko, time_limit_ms, memory_limit_mb,
    checker_kind, float_tolerance, content_hash
)
SELECT id, 1, statement_ko, time_limit_ms, memory_limit_mb, checker_kind,
       CASE WHEN checker_kind = 'float' THEN COALESCE(float_tolerance, 0.000001) END,
       digest(
           statement_ko || E'\x1f' || time_limit_ms::text || E'\x1f' ||
           memory_limit_mb::text || E'\x1f' || checker_kind,
           'sha256'
       )
FROM problems;

UPDATE problems problem
SET current_revision_id = revision.id
FROM problem_revisions revision
WHERE revision.problem_id = problem.id AND revision.version = 1;

ALTER TABLE problems
    ADD CHECK (status <> 'published' OR current_revision_id IS NOT NULL);

ALTER TABLE problem_test_cases
    ADD COLUMN group_key text NOT NULL DEFAULT 'main'
        CHECK (group_key ~ '^[a-z0-9][a-z0-9_-]{0,31}$');

INSERT INTO problem_test_cases (
    problem_id, ordinal, input, expected_output, visibility, score_weight, group_key
)
SELECT id, 2, E'-5 7\n', E'2\n', 'hidden', 1, 'main'
FROM problems WHERE slug = 'alpha-pair-sum';

CREATE TABLE submissions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    problem_id uuid NOT NULL REFERENCES problems(id),
    problem_revision_id uuid NOT NULL REFERENCES problem_revisions(id),
    language text NOT NULL CHECK (language IN ('cpp20', 'python3', 'java21')),
    source text NOT NULL CHECK (octet_length(source) BETWEEN 1 AND 100000),
    idempotency_key uuid NOT NULL,
    status text NOT NULL DEFAULT 'QUEUED' CHECK (status IN (
        'QUEUED', 'COMPILING', 'RUNNING', 'ACCEPTED', 'WRONG_ANSWER',
        'PARTIAL_ACCEPTED', 'TIME_LIMIT_EXCEEDED', 'MEMORY_LIMIT_EXCEEDED',
        'OUTPUT_LIMIT_EXCEEDED', 'RUNTIME_ERROR', 'COMPILE_ERROR',
        'SYSTEM_ERROR', 'CANCELLED'
    )),
    score smallint CHECK (score BETWEEN 0 AND 100),
    compile_output text CHECK (octet_length(compile_output) <= 16384),
    assistance_level text NOT NULL DEFAULT 'none' CHECK (
        assistance_level IN ('none', 'hint', 'guided', 'ai_assisted')
    ),
    created_at timestamptz NOT NULL DEFAULT now(),
    judged_at timestamptz,
    UNIQUE (user_id, idempotency_key),
    CHECK ((status IN (
        'ACCEPTED', 'WRONG_ANSWER', 'PARTIAL_ACCEPTED', 'TIME_LIMIT_EXCEEDED',
        'MEMORY_LIMIT_EXCEEDED', 'OUTPUT_LIMIT_EXCEEDED', 'RUNTIME_ERROR',
        'COMPILE_ERROR', 'SYSTEM_ERROR', 'CANCELLED'
    )) = (judged_at IS NOT NULL))
);

CREATE INDEX submissions_user_time_idx ON submissions (user_id, created_at DESC);
CREATE INDEX submissions_problem_time_idx ON submissions (problem_id, created_at DESC);

CREATE TABLE submission_events (
    id bigserial PRIMARY KEY,
    submission_id uuid NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    status text NOT NULL,
    safe_message text CHECK (char_length(safe_message) <= 500),
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX submission_events_stream_idx ON submission_events (submission_id, id);

CREATE TABLE judge_jobs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    submission_id uuid NOT NULL UNIQUE REFERENCES submissions(id) ON DELETE CASCADE,
    protocol_version smallint NOT NULL DEFAULT 1 CHECK (protocol_version = 1),
    status text NOT NULL DEFAULT 'ready' CHECK (status IN ('ready', 'leased', 'done', 'dead')),
    attempt_count smallint NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts smallint NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 10),
    available_at timestamptz NOT NULL DEFAULT now(),
    lease_owner text,
    lease_token uuid,
    lease_expires_at timestamptz,
    last_error text CHECK (char_length(last_error) <= 1000),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'leased') = (lease_owner IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL))
);

CREATE INDEX judge_jobs_available_idx
    ON judge_jobs (available_at, created_at)
    WHERE status = 'ready';
CREATE INDEX judge_jobs_expired_lease_idx
    ON judge_jobs (lease_expires_at)
    WHERE status = 'leased';

CREATE TABLE judge_workers (
    worker_id text PRIMARY KEY CHECK (char_length(worker_id) BETWEEN 3 AND 120),
    protocol_version smallint NOT NULL CHECK (protocol_version = 1),
    image_reference text NOT NULL CHECK (char_length(image_reference) BETWEEN 1 AND 300),
    status text NOT NULL CHECK (status IN ('ready', 'busy', 'draining')),
    current_job_id uuid REFERENCES judge_jobs(id),
    last_heartbeat_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE source_drafts (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE CASCADE,
    language text NOT NULL CHECK (language IN ('cpp20', 'python3', 'java21')),
    source text NOT NULL CHECK (octet_length(source) <= 100000),
    revision_count integer NOT NULL DEFAULT 1 CHECK (revision_count > 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, problem_id)
);

CREATE TABLE rejudge_requests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    problem_id uuid NOT NULL REFERENCES problems(id),
    requested_by uuid NOT NULL REFERENCES users(id),
    reason text NOT NULL CHECK (char_length(reason) BETWEEN 10 AND 500),
    created_at timestamptz NOT NULL DEFAULT now()
);

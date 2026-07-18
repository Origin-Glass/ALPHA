CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE problems (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    statement_ko text NOT NULL CHECK (char_length(statement_ko) BETWEEN 1 AND 50000),
    difficulty smallint NOT NULL CHECK (difficulty BETWEEN 0 AND 30),
    learning_axis text NOT NULL CHECK (
        learning_axis IN ('algorithmic_reasoning', 'code_literacy', 'docs_learning', 'independent_coding')
    ),
    status text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published', 'archived')),
    source_kind text NOT NULL DEFAULT 'original' CHECK (source_kind IN ('original', 'licensed', 'external_metadata')),
    source_url text,
    time_limit_ms integer NOT NULL CHECK (time_limit_ms BETWEEN 100 AND 30000),
    memory_limit_mb integer NOT NULL CHECK (memory_limit_mb BETWEEN 16 AND 2048),
    published_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'published') = (published_at IS NOT NULL))
);

CREATE INDEX problems_published_catalog_idx
    ON problems (published_at DESC, id DESC)
    WHERE status = 'published';

CREATE TABLE problem_tags (
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE CASCADE,
    tag text NOT NULL CHECK (tag ~ '^[a-z0-9_]+$'),
    label_ko text NOT NULL CHECK (char_length(label_ko) BETWEEN 1 AND 40),
    PRIMARY KEY (problem_id, tag)
);

CREATE TABLE problem_test_cases (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE CASCADE,
    ordinal integer NOT NULL CHECK (ordinal > 0),
    input text NOT NULL,
    expected_output text NOT NULL,
    visibility text NOT NULL CHECK (visibility IN ('sample', 'hidden')),
    score_weight integer NOT NULL DEFAULT 1 CHECK (score_weight > 0),
    UNIQUE (problem_id, ordinal)
);

CREATE INDEX problem_test_cases_judge_idx
    ON problem_test_cases (problem_id, ordinal);

CREATE TABLE external_problem_metadata_cache (
    provider text NOT NULL CHECK (provider IN ('solved_ac')),
    external_problem_id text NOT NULL,
    payload jsonb NOT NULL,
    fetched_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    source_url text NOT NULL,
    state text NOT NULL CHECK (state IN ('fresh', 'stale', 'blocked')),
    PRIMARY KEY (provider, external_problem_id),
    CHECK (expires_at >= fetched_at)
);

COMMENT ON TABLE external_problem_metadata_cache IS
    '외부 문제 메타데이터의 출처·갱신 시각·신선도를 보존하는 캐시. 실패 시 값을 임의 생성하지 않는다.';

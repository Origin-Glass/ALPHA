CREATE TABLE contests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    description_ko text NOT NULL CHECK (char_length(description_ko) BETWEEN 1 AND 2000),
    visibility text NOT NULL CHECK (visibility IN ('public', 'private', 'organization')),
    organization_id uuid REFERENCES organizations(id) ON DELETE CASCADE,
    join_code_hash bytea CHECK (join_code_hash IS NULL OR octet_length(join_code_hash) = 32),
    scoring_mode text NOT NULL CHECK (scoring_mode IN ('icpc', 'score')),
    starts_at timestamptz NOT NULL,
    freezes_at timestamptz,
    ends_at timestamptz NOT NULL,
    status text NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published', 'cancelled')),
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at),
    CHECK (freezes_at IS NULL OR (freezes_at > starts_at AND freezes_at < ends_at)),
    CHECK ((visibility = 'organization') = (organization_id IS NOT NULL)),
    CHECK ((visibility = 'private') = (join_code_hash IS NOT NULL))
);

CREATE INDEX contests_catalog_idx ON contests (starts_at DESC, id DESC) WHERE status = 'published';

CREATE TABLE contest_problems (
    contest_id uuid NOT NULL REFERENCES contests(id) ON DELETE CASCADE,
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE RESTRICT,
    position integer NOT NULL CHECK (position > 0),
    label text NOT NULL CHECK (label ~ '^[A-Z]{1,3}$'),
    points integer NOT NULL DEFAULT 100 CHECK (points BETWEEN 1 AND 1000),
    PRIMARY KEY (contest_id, problem_id),
    UNIQUE (contest_id, position),
    UNIQUE (contest_id, label)
);

CREATE TABLE contest_registrations (
    contest_id uuid NOT NULL REFERENCES contests(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disqualified', 'withdrawn')),
    registered_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (contest_id, user_id)
);

ALTER TABLE submissions ADD COLUMN contest_id uuid REFERENCES contests(id);
CREATE INDEX submissions_contest_scoreboard_idx
    ON submissions (contest_id, user_id, problem_id, created_at)
    WHERE contest_id IS NOT NULL AND run_kind = 'formal';

CREATE TABLE contest_rating_history (
    contest_id uuid NOT NULL REFERENCES contests(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    rank integer NOT NULL CHECK (rank > 0),
    rating_before integer NOT NULL CHECK (rating_before BETWEEN 0 AND 1000),
    rating_delta integer NOT NULL CHECK (rating_delta BETWEEN -200 AND 200),
    rating_after integer NOT NULL CHECK (rating_after BETWEEN 0 AND 1000),
    finalized_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (contest_id, user_id),
    CHECK (rating_after = rating_before + rating_delta)
);

INSERT INTO contests (
    slug, title_ko, description_ko, visibility, scoring_mode,
    starts_at, freezes_at, ends_at
) VALUES (
    'alpha-launch-sprint', 'ALPHA 시작 스프린트',
    '빠른 피드백과 문서 활용을 함께 확인하는 공개 대회입니다. 문제별 첫 정답과 오답 페널티가 점수판에 반영됩니다.',
    'public', 'icpc', '2026-07-18T00:00:00+09:00', '2026-07-19T18:00:00+09:00', '2026-07-20T00:00:00+09:00'
);

INSERT INTO contest_problems (contest_id, problem_id, position, label, points) VALUES
    ((SELECT id FROM contests WHERE slug = 'alpha-launch-sprint'), (SELECT id FROM problems WHERE slug = 'alpha-pair-sum'), 1, 'A', 100),
    ((SELECT id FROM contests WHERE slug = 'alpha-launch-sprint'), (SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 2, 'B', 100);

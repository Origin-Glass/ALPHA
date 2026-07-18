ALTER TABLE external_problem_metadata_cache
    ADD COLUMN raw_level smallint,
    ADD COLUMN normalized_tier text,
    ADD COLUMN normalized_tags jsonb NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN response_hash text,
    ADD COLUMN provider_revision text,
    ADD COLUMN last_attempt_at timestamptz,
    ADD COLUMN last_error text,
    ADD CHECK (raw_level IS NULL OR raw_level >= 0),
    ADD CHECK (jsonb_typeof(normalized_tags) = 'array'),
    ADD CHECK (response_hash IS NULL OR response_hash ~ '^[0-9a-f]{64}$');

UPDATE external_problem_metadata_cache
SET last_attempt_at = fetched_at
WHERE last_attempt_at IS NULL;

CREATE TABLE problem_external_links (
    problem_id uuid PRIMARY KEY REFERENCES problems(id) ON DELETE CASCADE,
    provider text NOT NULL CHECK (provider IN ('solved_ac')),
    external_problem_id text NOT NULL CHECK (external_problem_id ~ '^[0-9]{1,12}$'),
    attribution_ko text NOT NULL CHECK (char_length(attribution_ko) BETWEEN 1 AND 200),
    UNIQUE (provider, external_problem_id)
);

CREATE INDEX problem_external_links_lookup_idx
    ON problem_external_links (provider, external_problem_id);

INSERT INTO problems (
    slug, title_ko, statement_ko, difficulty, learning_axis, status, source_kind,
    time_limit_ms, memory_limit_mb, published_at
) VALUES (
    'alpha-pair-sum',
    '두 수의 합',
    E'두 정수 A와 B를 입력받아 합을 출력하세요.\n\n입력: 한 줄에 두 정수가 공백으로 주어집니다.\n출력: 두 정수의 합을 출력합니다.',
    1,
    'algorithmic_reasoning',
    'published',
    'original',
    1000,
    256,
    now()
);

INSERT INTO problem_tags (problem_id, tag, label_ko)
SELECT id, 'implementation', '구현'
FROM problems WHERE slug = 'alpha-pair-sum';

INSERT INTO problem_test_cases (problem_id, ordinal, input, expected_output, visibility)
SELECT id, 1, E'1 2\n', E'3\n', 'sample'
FROM problems WHERE slug = 'alpha-pair-sum';

INSERT INTO problems (
    slug, title_ko, statement_ko, difficulty, learning_axis, status, source_kind,
    source_url, time_limit_ms, memory_limit_mb, published_at
) VALUES (
    'boj-1000-link',
    'BOJ 1000 외부 연계',
    '문제 원문은 출처 링크에서 확인합니다. ALPHA는 허용된 공개 메타데이터만 별도로 동기화합니다.',
    0,
    'algorithmic_reasoning',
    'published',
    'external_metadata',
    'https://www.acmicpc.net/problem/1000',
    2000,
    512,
    now()
);

INSERT INTO problem_external_links (problem_id, provider, external_problem_id, attribution_ko)
SELECT id, 'solved_ac', '1000', '난이도와 알고리즘 태그 출처: solved.ac'
FROM problems WHERE slug = 'boj-1000-link';

INSERT INTO external_problem_metadata_cache (
    provider, external_problem_id, payload, fetched_at, expires_at, source_url, state,
    normalized_tags, provider_revision, last_attempt_at, last_error
) VALUES (
    'solved_ac',
    '1000',
    '{}'::jsonb,
    TIMESTAMPTZ '2026-04-28T00:00:00Z',
    TIMESTAMPTZ '2026-04-28T00:00:00Z',
    'https://solved.ac/api/v3/problem/show?problemId=1000',
    'blocked',
    '[]'::jsonb,
    'api-v3',
    TIMESTAMPTZ '2026-04-28T00:00:00Z',
    '공식 서비스 종료 이후 운영 정책상 실시간 동기화를 확인할 수 없습니다'
)
ON CONFLICT (provider, external_problem_id) DO NOTHING;

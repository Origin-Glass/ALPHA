CREATE TABLE community_posts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    author_id uuid NOT NULL REFERENCES users(id),
    problem_id uuid REFERENCES problems(id) ON DELETE SET NULL,
    kind text NOT NULL CHECK (kind IN ('notice', 'question', 'discussion')),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 160),
    body_ko text NOT NULL CHECK (char_length(body_ko) BETWEEN 1 AND 10000),
    status text NOT NULL DEFAULT 'published' CHECK (status IN ('published', 'hidden', 'locked', 'deleted')),
    pinned boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (kind <> 'notice' OR problem_id IS NULL)
);

CREATE INDEX community_posts_catalog_idx
    ON community_posts (pinned DESC, created_at DESC, id DESC)
    WHERE status IN ('published', 'locked');
CREATE INDEX community_posts_problem_idx
    ON community_posts (problem_id, created_at DESC)
    WHERE problem_id IS NOT NULL AND status IN ('published', 'locked');

CREATE TABLE community_answers (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    post_id uuid NOT NULL REFERENCES community_posts(id) ON DELETE CASCADE,
    author_id uuid NOT NULL REFERENCES users(id),
    body_ko text NOT NULL CHECK (char_length(body_ko) BETWEEN 1 AND 10000),
    status text NOT NULL DEFAULT 'published' CHECK (status IN ('published', 'hidden', 'deleted')),
    accepted_by uuid REFERENCES users(id),
    accepted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((accepted_by IS NULL) = (accepted_at IS NULL))
);

CREATE UNIQUE INDEX community_answers_one_accepted_idx
    ON community_answers (post_id) WHERE accepted_at IS NOT NULL;
CREATE INDEX community_answers_post_idx
    ON community_answers (post_id, created_at, id) WHERE status = 'published';

CREATE TABLE community_reports (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    reporter_id uuid NOT NULL REFERENCES users(id),
    target_type text NOT NULL CHECK (target_type IN ('post', 'answer')),
    post_id uuid REFERENCES community_posts(id) ON DELETE CASCADE,
    answer_id uuid REFERENCES community_answers(id) ON DELETE CASCADE,
    reason text NOT NULL CHECK (reason IN ('spam', 'abuse', 'solution_leak', 'privacy', 'other')),
    detail_ko text NOT NULL DEFAULT '' CHECK (char_length(detail_ko) <= 2000),
    status text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'dismissed')),
    handled_by uuid REFERENCES users(id),
    resolution_note_ko text CHECK (char_length(resolution_note_ko) <= 2000),
    created_at timestamptz NOT NULL DEFAULT now(),
    handled_at timestamptz,
    CHECK (
        (target_type = 'post' AND post_id IS NOT NULL AND answer_id IS NULL)
        OR (target_type = 'answer' AND answer_id IS NOT NULL AND post_id IS NULL)
    ),
    CHECK ((handled_by IS NULL) = (handled_at IS NULL)),
    CHECK ((status = 'open') = (handled_at IS NULL))
);

CREATE UNIQUE INDEX community_reports_one_open_post_idx
    ON community_reports (reporter_id, post_id) WHERE status = 'open' AND post_id IS NOT NULL;
CREATE UNIQUE INDEX community_reports_one_open_answer_idx
    ON community_reports (reporter_id, answer_id) WHERE status = 'open' AND answer_id IS NOT NULL;
CREATE INDEX community_reports_queue_idx
    ON community_reports (created_at, id) WHERE status = 'open';

INSERT INTO community_posts (author_id, kind, title_ko, body_ko, pinned)
SELECT id, 'notice', 'ALPHA 커뮤니티 이용 안내',
       '질문에는 시도한 방법과 막힌 지점을 함께 적어 주세요. 대회 중 정답 코드 공유와 개인정보 노출은 금지됩니다.', true
FROM users WHERE handle = 'alpha-demo-instructor';

INSERT INTO community_posts (author_id, problem_id, kind, title_ko, body_ko)
SELECT instructor.id, problem.id, 'question', '두 수의 합에서 입력을 읽는 방법',
       '공백으로 구분된 두 정수를 안전하게 읽고 합을 출력하려면 어떤 순서로 확인하면 좋을까요? 정답 전체보다 입력 계약을 설명해 주세요.'
FROM users instructor
JOIN problems problem ON problem.slug = 'alpha-pair-sum'
WHERE instructor.handle = 'alpha-demo-instructor';

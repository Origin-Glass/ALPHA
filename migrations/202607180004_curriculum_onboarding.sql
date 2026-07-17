CREATE TABLE learning_tracks (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,47}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 100),
    description_ko text NOT NULL CHECK (char_length(description_ko) BETWEEN 1 AND 500),
    primary_axis text NOT NULL CHECK (
        primary_axis IN ('algorithmic_reasoning', 'code_literacy', 'docs_learning', 'independent_coding')
    ),
    status text NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published', 'archived')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE curriculum_units (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    track_id uuid NOT NULL REFERENCES learning_tracks(id) ON DELETE CASCADE,
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    summary_ko text NOT NULL CHECK (char_length(summary_ko) BETWEEN 1 AND 500),
    activity_kind text NOT NULL CHECK (
        activity_kind IN ('algorithm', 'code_reading', 'debugging', 'docs_project', 'independent_build')
    ),
    position integer NOT NULL CHECK (position > 0),
    estimated_minutes integer NOT NULL CHECK (estimated_minutes BETWEEN 5 AND 240),
    status text NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published', 'archived')),
    UNIQUE (track_id, position)
);

CREATE TABLE curriculum_prerequisites (
    unit_id uuid NOT NULL REFERENCES curriculum_units(id) ON DELETE CASCADE,
    prerequisite_unit_id uuid NOT NULL REFERENCES curriculum_units(id) ON DELETE CASCADE,
    PRIMARY KEY (unit_id, prerequisite_unit_id),
    CHECK (unit_id <> prerequisite_unit_id)
);

CREATE TABLE diagnostic_questions (
    question_key text PRIMARY KEY CHECK (question_key ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    axis text NOT NULL CHECK (
        axis IN ('algorithmic_reasoning', 'code_literacy', 'docs_learning', 'independent_coding')
    ),
    prompt_ko text NOT NULL CHECK (char_length(prompt_ko) BETWEEN 1 AND 1000),
    options_ko jsonb NOT NULL CHECK (jsonb_typeof(options_ko) = 'array'),
    option_count smallint NOT NULL CHECK (option_count BETWEEN 2 AND 8),
    correct_option smallint NOT NULL CHECK (correct_option >= 0 AND correct_option < option_count),
    position integer NOT NULL UNIQUE CHECK (position > 0),
    active boolean NOT NULL DEFAULT true,
    CHECK (jsonb_array_length(options_ko) = option_count)
);

CREATE TABLE diagnostic_attempts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scores jsonb NOT NULL,
    recommended_track_id uuid NOT NULL REFERENCES learning_tracks(id),
    completed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE diagnostic_answers (
    attempt_id uuid NOT NULL REFERENCES diagnostic_attempts(id) ON DELETE CASCADE,
    question_key text NOT NULL REFERENCES diagnostic_questions(question_key),
    selected_option smallint NOT NULL,
    correct boolean NOT NULL,
    PRIMARY KEY (attempt_id, question_key)
);

CREATE TABLE user_learning_paths (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    track_id uuid NOT NULL REFERENCES learning_tracks(id),
    source_attempt_id uuid NOT NULL REFERENCES diagnostic_attempts(id),
    selected_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_unit_progress (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    unit_id uuid NOT NULL REFERENCES curriculum_units(id) ON DELETE CASCADE,
    status text NOT NULL CHECK (status IN ('started', 'completed')),
    mastery_score smallint CHECK (mastery_score BETWEEN 0 AND 100),
    assistance_level text CHECK (assistance_level IN ('none', 'hint', 'guided', 'ai_assisted')),
    started_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    PRIMARY KEY (user_id, unit_id),
    CHECK ((status = 'completed') = (completed_at IS NOT NULL))
);

INSERT INTO learning_tracks (slug, title_ko, description_ko, primary_axis) VALUES
    ('algorithm-foundations', '알고리즘 추론 기초', '경계 조건과 불변식으로 풀이 근거를 세웁니다.', 'algorithmic_reasoning'),
    ('code-reader', '코드 리더', '다른 사람의 코드에서 흐름, 불변식, 결함을 찾습니다.', 'code_literacy'),
    ('docs-builder', '문서로 만드는 개발자', '공식 문서의 계약을 읽고 작동하는 결과를 만듭니다.', 'docs_learning'),
    ('independent-builder', '독립 코딩 빌더', '도움 수준을 스스로 조절하며 증거로 문제를 해결합니다.', 'independent_coding');

INSERT INTO curriculum_units (track_id, slug, title_ko, summary_ko, activity_kind, position, estimated_minutes) VALUES
    ((SELECT id FROM learning_tracks WHERE slug = 'algorithm-foundations'), 'reason-from-constraints', '제약에서 풀이 끌어내기', '입력 범위와 시간 제한을 알고리즘 선택 근거로 바꿉니다.', 'algorithm', 1, 25),
    ((SELECT id FROM learning_tracks WHERE slug = 'algorithm-foundations'), 'prove-loop-invariant', '반복문 불변식 증명', '코드 실행 전·중·후에 유지되는 조건을 설명합니다.', 'algorithm', 2, 35),
    ((SELECT id FROM learning_tracks WHERE slug = 'code-reader'), 'trace-control-flow', '제어 흐름 추적', '실행하지 않고 분기와 반복의 상태 변화를 추적합니다.', 'code_reading', 1, 20),
    ((SELECT id FROM learning_tracks WHERE slug = 'code-reader'), 'find-boundary-bugs', '경계 값 버그 찾기', '하나 부족하거나 너무 많이 반복하는 코드를 반례로 검증합니다.', 'debugging', 2, 25),
    ((SELECT id FROM learning_tracks WHERE slug = 'code-reader'), 'review-with-invariants', '불변식으로 리뷰하기', '취향이 아닌 정확성 근거로 코드 리뷰를 작성합니다.', 'code_reading', 3, 35),
    ((SELECT id FROM learning_tracks WHERE slug = 'docs-builder'), 'read-api-contract', 'API 계약 찾기', '인증, 입력, 오류, 제한을 공식 문서에서 찾습니다.', 'docs_project', 1, 25),
    ((SELECT id FROM learning_tracks WHERE slug = 'docs-builder'), 'build-from-docs', '문서만으로 연동 구현', '예제 복사가 아닌 계약 해석으로 작동하는 연동을 만듭니다.', 'docs_project', 2, 50),
    ((SELECT id FROM learning_tracks WHERE slug = 'independent-builder'), 'debug-with-evidence', '증거로 디버깅', '가설, 관찰, 반증, 수정의 순서로 문제를 좁힙니다.', 'independent_build', 1, 30),
    ((SELECT id FROM learning_tracks WHERE slug = 'independent-builder'), 'build-without-scaffold', '스캐폴드 없이 만들기', '목표와 검증 기준만으로 작은 기능을 완성합니다.', 'independent_build', 2, 60);

INSERT INTO curriculum_prerequisites (unit_id, prerequisite_unit_id)
SELECT next.id, previous.id
FROM curriculum_units next
JOIN curriculum_units previous ON previous.track_id = next.track_id AND previous.position = next.position - 1;

INSERT INTO diagnostic_questions
    (question_key, axis, prompt_ko, options_ko, option_count, correct_option, position)
VALUES
    ('algorithm-boundary', 'algorithmic_reasoning', '폐구간 [left, right]를 유지하는 이분 탐색에서 탐색을 계속하는 조건은?', '["left < right", "left <= right", "left == right"]', 3, 1, 1),
    ('code-reading-loop', 'code_literacy', '`for i in 0..len-1`이 len=0일 때 만들 수 있는 핵심 문제는?', '["첫 원소를 두 번 읽음", "마지막 원소만 잃음", "0에서 1을 빼며 범위가 깨짐"]', 3, 2, 2),
    ('docs-contract', 'docs_learning', '외부 API 연동 전 가장 먼저 고정해야 할 정보는?', '["예제의 변수 이름", "인증·입력·오류·제한 계약", "README의 문장 수"]', 3, 1, 3),
    ('independent-debugging', 'independent_coding', '테스트가 실패했을 때 독립적인 첫 행동은?', '["실패를 재현하고 가설을 하나씩 검증", "전체 코드를 즉시 재작성", "정답 코드를 먼저 검색"]', 3, 0, 4);

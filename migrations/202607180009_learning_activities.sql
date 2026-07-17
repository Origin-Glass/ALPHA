CREATE TABLE learning_modules (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    track_id uuid NOT NULL REFERENCES learning_tracks(id) ON DELETE CASCADE,
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    goal_ko text NOT NULL CHECK (char_length(goal_ko) BETWEEN 1 AND 1000),
    position integer NOT NULL CHECK (position > 0),
    UNIQUE (track_id, position)
);

CREATE TABLE lessons (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    module_id uuid NOT NULL REFERENCES learning_modules(id) ON DELETE CASCADE,
    unit_id uuid REFERENCES curriculum_units(id) ON DELETE SET NULL,
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    goal_ko text NOT NULL CHECK (char_length(goal_ko) BETWEEN 1 AND 1000),
    mastery_criteria_ko text NOT NULL CHECK (char_length(mastery_criteria_ko) BETWEEN 1 AND 1000),
    remediation_ko text NOT NULL CHECK (char_length(remediation_ko) BETWEEN 1 AND 1000),
    position integer NOT NULL CHECK (position > 0),
    UNIQUE (module_id, position)
);

CREATE TABLE documentation_resources (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 200),
    url text NOT NULL UNIQUE CHECK (url ~ '^https://'),
    publisher text NOT NULL CHECK (char_length(publisher) BETWEEN 1 AND 100),
    resource_kind text NOT NULL CHECK (resource_kind IN ('official_documentation', 'licensed_reading')),
    license_note_ko text NOT NULL CHECK (char_length(license_note_ko) BETWEEN 1 AND 500)
);

CREATE TABLE lesson_resources (
    lesson_id uuid NOT NULL REFERENCES lessons(id) ON DELETE CASCADE,
    resource_id uuid NOT NULL REFERENCES documentation_resources(id) ON DELETE RESTRICT,
    reading_goal_ko text NOT NULL CHECK (char_length(reading_goal_ko) BETWEEN 1 AND 1000),
    position integer NOT NULL CHECK (position > 0),
    PRIMARY KEY (lesson_id, resource_id),
    UNIQUE (lesson_id, position)
);

CREATE TABLE learning_activities (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    lesson_id uuid REFERENCES lessons(id) ON DELETE SET NULL,
    unit_id uuid REFERENCES curriculum_units(id) ON DELETE SET NULL,
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    kind text NOT NULL CHECK (kind IN (
        'predict_output', 'trace_state', 'explain_behavior', 'identify_invariant',
        'locate_bug', 'compare_implementations', 'estimate_complexity',
        'reconstruct_code', 'assess_tests', 'code_review', 'docs_checkpoint'
    )),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    instructions_ko text NOT NULL CHECK (char_length(instructions_ko) BETWEEN 1 AND 5000),
    starter_code text CHECK (octet_length(starter_code) <= 50000),
    public_response_schema jsonb NOT NULL CHECK (jsonb_typeof(public_response_schema) = 'object'),
    evaluator_kind text NOT NULL CHECK (evaluator_kind IN ('exact_text', 'single_choice', 'structured_fields', 'ordered_tokens')),
    evaluator_config jsonb NOT NULL CHECK (jsonb_typeof(evaluator_config) = 'object'),
    estimated_minutes integer NOT NULL CHECK (estimated_minutes BETWEEN 3 AND 240),
    full_explanation_allowed boolean NOT NULL DEFAULT false,
    status text NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published', 'archived')),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (lesson_id IS NOT NULL OR unit_id IS NOT NULL)
);

COMMENT ON COLUMN learning_activities.evaluator_config IS
    '정답과 구조화 평가 기준. 공개 API 응답에 포함하지 않는다.';

CREATE TABLE activity_assistance_content (
    activity_id uuid NOT NULL REFERENCES learning_activities(id) ON DELETE CASCADE,
    level smallint NOT NULL CHECK (level BETWEEN 1 AND 6),
    kind text NOT NULL CHECK (kind IN (
        'restatement', 'prerequisites', 'diagnostic_question',
        'documentation_pointer', 'limited_hint', 'error_category'
    )),
    content_ko text NOT NULL CHECK (char_length(content_ko) BETWEEN 1 AND 2000),
    PRIMARY KEY (activity_id, level)
);

CREATE TABLE activity_progress (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    activity_id uuid NOT NULL REFERENCES learning_activities(id) ON DELETE CASCADE,
    status text NOT NULL DEFAULT 'started' CHECK (status IN ('started', 'completed')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    failed_attempt_count integer NOT NULL DEFAULT 0 CHECK (failed_attempt_count >= 0),
    revision_count integer NOT NULL DEFAULT 0 CHECK (revision_count >= 0),
    max_assistance_level smallint NOT NULL DEFAULT 0 CHECK (max_assistance_level BETWEEN 0 AND 9),
    best_score smallint NOT NULL DEFAULT 0 CHECK (best_score BETWEEN 0 AND 100),
    mastery_class text CHECK (mastery_class IN ('independent', 'assisted', 'reviewed', 'explained')),
    explicit_reflection text CHECK (char_length(explicit_reflection) <= 2000),
    started_at timestamptz NOT NULL DEFAULT now(),
    last_active_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    PRIMARY KEY (user_id, activity_id),
    CHECK ((status = 'completed') = (completed_at IS NOT NULL))
);

CREATE TABLE activity_attempts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    activity_id uuid NOT NULL REFERENCES learning_activities(id) ON DELETE CASCADE,
    response jsonb NOT NULL CHECK (jsonb_typeof(response) = 'object'),
    score smallint NOT NULL CHECK (score BETWEEN 0 AND 100),
    passed boolean NOT NULL,
    max_assistance_level smallint NOT NULL CHECK (max_assistance_level BETWEEN 0 AND 9),
    mastery_class text CHECK (mastery_class IN ('independent', 'assisted', 'reviewed', 'explained')),
    elapsed_seconds integer NOT NULL CHECK (elapsed_seconds BETWEEN 0 AND 86400),
    reflection text CHECK (char_length(reflection) <= 2000),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (passed = (score >= 80)),
    CHECK ((mastery_class IS NOT NULL) = passed)
);

CREATE INDEX activity_attempts_user_time_idx ON activity_attempts (user_id, created_at DESC);

CREATE TABLE assistance_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    activity_id uuid NOT NULL REFERENCES learning_activities(id) ON DELETE CASCADE,
    level smallint NOT NULL CHECK (level BETWEEN 1 AND 9),
    assistance_kind text NOT NULL CHECK (assistance_kind IN (
        'restatement', 'prerequisites', 'diagnostic_question', 'documentation_pointer',
        'limited_hint', 'error_category', 'reasoning_critique', 'code_review', 'full_explanation'
    )),
    provider text NOT NULL CHECK (provider IN ('deterministic', 'disabled_ai')),
    elapsed_seconds integer NOT NULL CHECK (elapsed_seconds BETWEEN 0 AND 86400),
    created_at timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE assistance_events IS
    '도움 단계와 시간만 기록한다. 프롬프트, 응답, 숨은 사고 과정은 저장하지 않는다.';

INSERT INTO learning_modules (track_id, slug, title_ko, goal_ko, position) VALUES
    ((SELECT id FROM learning_tracks WHERE slug = 'code-reader'), 'read-and-trace', '읽고 추적하기', '실행 전에 제어 흐름과 상태를 정확히 예측한다.', 1),
    ((SELECT id FROM learning_tracks WHERE slug = 'code-reader'), 'debug-and-review', '결함을 분류하고 리뷰하기', '반례와 불변식으로 버그를 분류하고 수정 방향을 설명한다.', 2),
    ((SELECT id FROM learning_tracks WHERE slug = 'docs-builder'), 'python-url-contracts', 'Python URL 계약 읽기', '공식 문서에서 URL 분해와 쿼리 변환 계약을 찾아 설명한다.', 1),
    ((SELECT id FROM learning_tracks WHERE slug = 'docs-builder'), 'python-url-capstone', '문서로 URL 도구 완성하기', '공식 문서만 참고해 실행 가능한 URL 정규화 도구를 완성한다.', 2);

INSERT INTO lessons (module_id, unit_id, slug, title_ko, goal_ko, mastery_criteria_ko, remediation_ko, position) VALUES
    ((SELECT id FROM learning_modules WHERE slug = 'read-and-trace'), (SELECT id FROM curriculum_units WHERE slug = 'trace-control-flow'), 'trace-python-state', '상태를 표로 추적하기', '중첩 반복문의 각 반복 뒤 상태를 기록한다.', '최종 출력과 중간 상태 필드를 모두 80점 이상으로 맞힌다.', '변경되는 변수만 열로 둔 추적표를 만든 뒤 예제를 한 반복씩 다시 확인한다.', 1),
    ((SELECT id FROM learning_modules WHERE slug = 'debug-and-review'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'classify-boundary-defects', '경계 결함 분류하기', '실패 반례와 오류 범주를 함께 찾는다.', '오류 위치, 분류, 최소 반례를 구조화해서 80점 이상으로 제출한다.', '원소가 0개, 1개, 2개인 입력으로 탐색 구간 변화를 다시 기록한다.', 1),
    ((SELECT id FROM learning_modules WHERE slug = 'python-url-contracts'), (SELECT id FROM curriculum_units WHERE slug = 'read-api-contract'), 'read-urllib-parse', 'urllib.parse 계약 찾기', 'urlsplit, parse_qsl, urlencode의 반환값과 보존 옵션을 문서에서 찾는다.', '문서 근거를 사용해 구성요소와 빈 값 보존 방식을 정확히 설명한다.', '공식 문서에서 반환형과 keep_blank_values 문장을 다시 찾아 입력·출력 예를 직접 만든다.', 1),
    ((SELECT id FROM learning_modules WHERE slug = 'python-url-capstone'), (SELECT id FROM curriculum_units WHERE slug = 'build-from-docs'), 'build-url-normalizer', 'URL 정규화 도구 만들기', 'URL 조각을 제거하고 쿼리 쌍을 안정적으로 정렬하는 프로그램을 구현한다.', '공개 예제와 비공개 경계 테스트를 격리 판정기에서 모두 통과한다.', '빈 값, 중복 키, fragment 없는 URL을 사용자 정의 입력으로 각각 재현한다.', 1);

INSERT INTO documentation_resources (title_ko, url, publisher, resource_kind, license_note_ko) VALUES
    ('urllib.parse — URL을 구성 요소로 분석하기', 'https://docs.python.org/3/library/urllib.parse.html', 'Python Software Foundation', 'official_documentation', '공식 문서로 연결하며 원문을 ALPHA에 복제하지 않습니다.'),
    ('sorted — 안정적인 정렬 내장 함수', 'https://docs.python.org/3/library/functions.html#sorted', 'Python Software Foundation', 'official_documentation', '공식 문서로 연결하며 원문을 ALPHA에 복제하지 않습니다.');

INSERT INTO lesson_resources (lesson_id, resource_id, reading_goal_ko, position) VALUES
    ((SELECT id FROM lessons WHERE slug = 'read-urllib-parse'), (SELECT id FROM documentation_resources WHERE url = 'https://docs.python.org/3/library/urllib.parse.html'), 'urlsplit 반환 필드, parse_qsl의 keep_blank_values, urlunsplit 계약을 찾으세요.', 1),
    ((SELECT id FROM lessons WHERE slug = 'build-url-normalizer'), (SELECT id FROM documentation_resources WHERE url = 'https://docs.python.org/3/library/urllib.parse.html'), '쿼리 문자열을 이름·값 쌍으로 보존하며 다시 조합하는 방법을 찾으세요.', 1),
    ((SELECT id FROM lessons WHERE slug = 'build-url-normalizer'), (SELECT id FROM documentation_resources WHERE url = 'https://docs.python.org/3/library/functions.html#sorted'), '동일 키의 원래 순서를 보존하는 안정 정렬 계약을 확인하세요.', 2);

INSERT INTO learning_activities
    (lesson_id, unit_id, slug, kind, title_ko, instructions_ko, starter_code, public_response_schema, evaluator_kind, evaluator_config, estimated_minutes)
VALUES
    ((SELECT id FROM lessons WHERE slug = 'trace-python-state'), (SELECT id FROM curriculum_units WHERE slug = 'trace-control-flow'), 'predict-nested-loop-output', 'predict_output', '실행 없이 출력 예측', '코드를 실행하지 말고 마지막에 출력되는 한 줄을 정확히 적으세요.', E'total = 0\nfor i in range(3):\n    total += i\nprint(total)', '{"type":"text","label":"최종 출력"}', 'exact_text', '{"expected":"3"}', 5),
    ((SELECT id FROM lessons WHERE slug = 'trace-python-state'), (SELECT id FROM curriculum_units WHERE slug = 'trace-control-flow'), 'trace-two-pointer-state', 'trace_state', '투 포인터 상태 추적', '반복문이 끝난 뒤 left, right, total 값을 각각 적으세요.', E'values = [2, 4, 7]\nleft, right, total = 0, 2, 0\nwhile left < right:\n    total += values[right] - values[left]\n    left += 1\n    right -= 1', '{"type":"fields","fields":[{"key":"left","label":"left"},{"key":"right","label":"right"},{"key":"total","label":"total"}]}', 'structured_fields', '{"expected":{"left":"1","right":"1","total":"5"}}', 8),
    ((SELECT id FROM lessons WHERE slug = 'trace-python-state'), (SELECT id FROM curriculum_units WHERE slug = 'trace-control-flow'), 'identify-loop-invariant', 'identify_invariant', '반복문 불변식 고르기', '각 반복 시작 시 반드시 참인 설명을 고르세요.', E'i = 0\ntotal = 0\nwhile i < len(values):\n    total += values[i]\n    i += 1', '{"type":"choice","options":[{"value":"a","label":"total은 values 전체의 합이다"},{"value":"b","label":"total은 values[0:i]의 합이다"},{"value":"c","label":"i는 항상 len(values)보다 작다"}]}', 'single_choice', '{"expected":"b"}', 7),
    ((SELECT id FROM lessons WHERE slug = 'classify-boundary-defects'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'locate-binary-search-bug', 'locate_bug', '이분 탐색 경계 버그 분류', '오류가 있는 줄, 오류 범주, 실패하는 최소 target을 구조화해서 제출하세요.', E'def contains(values, target):\n    left, right = 0, len(values) - 1\n    while left < right:\n        mid = (left + right) // 2\n        if values[mid] < target:\n            left = mid + 1\n        else:\n            right = mid - 1\n    return left < len(values) and values[left] == target\n\nvalues = [1, 3]', '{"type":"fields","fields":[{"key":"line","label":"오류 줄 번호"},{"key":"category","label":"오류 범주"},{"key":"counterexample","label":"실패 target"}]}', 'structured_fields', '{"expected":{"line":"8","category":"경계 축소 오류","counterexample":"3"}}', 12),
    ((SELECT id FROM lessons WHERE slug = 'classify-boundary-defects'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'compare-membership-implementations', 'compare_implementations', '구현 비교', '정렬된 배열에서 존재 여부만 확인할 때 모든 입력에 맞는 구현을 고르세요.', E'A: while left < right, found면 즉시 true\nB: while left <= right, found면 즉시 true\nC: while left <= right, found여도 계속 right = mid - 1', '{"type":"choice","options":[{"value":"a","label":"A만"},{"value":"b","label":"B만"},{"value":"c","label":"B와 C"}]}', 'single_choice', '{"expected":"c"}', 6),
    ((SELECT id FROM lessons WHERE slug = 'classify-boundary-defects'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'estimate-double-loop-complexity', 'estimate_complexity', '복잡도 추정', '안쪽 반복 횟수가 i일 때 전체 시간 복잡도를 고르세요.', E'for i in range(n):\n    for j in range(i):\n        visit(i, j)', '{"type":"choice","options":[{"value":"a","label":"O(n)"},{"value":"b","label":"O(n log n)"},{"value":"c","label":"O(n²)"}]}', 'single_choice', '{"expected":"c"}', 5),
    ((SELECT id FROM lessons WHERE slug = 'classify-boundary-defects'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'reconstruct-safe-loop', 'reconstruct_code', '빠진 조건 복원', '빈 배열에서도 안전하게 마지막 인덱스부터 순회하도록 빈칸 토큰을 순서대로 쉼표로 구분해 적으세요.', E'i = len(values) __ 1\nwhile i __ 0:\n    consume(values[i])\n    i __ 1', '{"type":"tokens","label":"세 토큰"}', 'ordered_tokens', '{"expected":["-",">=","-="]}', 7),
    ((SELECT id FROM lessons WHERE slug = 'classify-boundary-defects'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'assess-boundary-tests', 'assess_tests', '테스트 공백 찾기', '양수 배열 테스트만 있는 합계 함수에서 가장 먼저 추가할 회귀 테스트를 고르세요.', NULL, '{"type":"choice","options":[{"value":"a","label":"원소가 100개인 양수 배열"},{"value":"b","label":"빈 배열과 음수만 있는 배열"},{"value":"c","label":"같은 양수 배열을 한 번 더"}]}', 'single_choice', '{"expected":"b"}', 5),
    ((SELECT id FROM lessons WHERE slug = 'classify-boundary-defects'), (SELECT id FROM curriculum_units WHERE slug = 'find-boundary-bugs'), 'review-index-access', 'code_review', '작은 코드 리뷰', '위험 분류와 구체적인 실패 입력을 함께 적으세요.', E'def first(items):\n    return items[0]', '{"type":"fields","fields":[{"key":"risk","label":"위험 분류"},{"key":"failing_input","label":"실패 입력"}]}', 'structured_fields', '{"expected":{"risk":"빈 컬렉션 접근","failing_input":"[]"}}', 8),
    ((SELECT id FROM lessons WHERE slug = 'read-urllib-parse'), (SELECT id FROM curriculum_units WHERE slug = 'read-api-contract'), 'urllib-contract-checkpoint', 'docs_checkpoint', '공식 문서 계약 체크포인트', '빈 쿼리 값도 보존해 이름·값 쌍 목록을 얻는 호출을 고르세요.', NULL, '{"type":"choice","options":[{"value":"a","label":"parse_qs(query)"},{"value":"b","label":"parse_qsl(query, keep_blank_values=True)"},{"value":"c","label":"urlsplit(query).path"}]}', 'single_choice', '{"expected":"b"}', 8);

INSERT INTO activity_assistance_content (activity_id, level, kind, content_ko)
SELECT activity.id, assistance.level, assistance.kind, assistance.content
FROM learning_activities activity
CROSS JOIN (VALUES
    (1, 'restatement', '코드를 실행하지 않고 요구된 출력 또는 상태를 답하는 활동입니다.'),
    (2, 'prerequisites', '대입, 분기, 반복문, 인덱스 경계를 먼저 확인하세요.'),
    (3, 'diagnostic_question', '첫 반복이 끝난 직후 각 변수 값은 무엇인가요?'),
    (4, 'documentation_pointer', 'Python 언어 참조에서 range의 끝 값 제외 규칙을 확인해 보세요.'),
    (5, 'limited_hint', '변경되는 변수만 표의 열로 두고 반복마다 한 행씩 적으세요.'),
    (6, 'error_category', '정답이 다르면 반복 횟수 또는 경계 축소 범주를 먼저 점검하세요.')
) AS assistance(level, kind, content)
WHERE activity.kind <> 'docs_checkpoint';

INSERT INTO activity_assistance_content (activity_id, level, kind, content_ko)
SELECT activity.id, assistance.level, assistance.kind, assistance.content
FROM learning_activities activity
CROSS JOIN (VALUES
    (1, 'restatement', '공식 문서에서 빈 쿼리 값을 보존하는 호출 계약을 찾는 활동입니다.'),
    (2, 'prerequisites', 'URL의 query 구성요소와 이름·값 쌍의 차이를 구분하세요.'),
    (3, 'diagnostic_question', 'parse_qs와 parse_qsl의 반환 자료형은 어떻게 다른가요?'),
    (4, 'documentation_pointer', '연결된 urllib.parse 문서에서 parse_qsl과 keep_blank_values를 찾으세요.'),
    (5, 'limited_hint', '이름·값 쌍의 순서와 중복을 보존하는 반환형을 고르세요.'),
    (6, 'error_category', '오답이면 반환형 또는 빈 값 보존 옵션을 혼동했을 가능성이 큽니다.')
) AS assistance(level, kind, content)
WHERE activity.kind = 'docs_checkpoint';

INSERT INTO problems (
    slug, title_ko, statement_ko, difficulty, learning_axis, status, source_kind,
    time_limit_ms, memory_limit_mb, checker_kind, published_at
) VALUES (
    'docs-url-normalizer',
    '문서로 만드는 URL 정규화기',
    E'Python 표준 라이브러리 공식 문서를 참고해 URL 정규화기를 작성하세요.\n\n입력 첫 줄에는 하나의 절대 URL이 주어집니다. 출력에는 다음 규칙을 적용한 URL을 한 줄로 출력합니다.\n\n1. scheme과 host는 소문자로 바꿉니다.\n2. fragment는 제거합니다.\n3. 쿼리 이름·값 쌍은 이름, 값 순서로 정렬합니다. 빈 값과 중복 쌍을 보존합니다.\n4. 나머지 path와 percent-encoding은 입력 표현을 유지합니다.\n\nPython 제출은 urllib.parse의 urlsplit, parse_qsl, urlencode, urlunsplit을 사용할 수 있습니다.',
    8, 'docs_learning', 'draft', 'original', 2000, 128, 'exact', NULL
);

INSERT INTO problem_tags (problem_id, tag, label_ko) VALUES
    ((SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 'documentation', '문서 활용'),
    ((SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 'parsing', '문자열 파싱'),
    ((SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 'sorting', '정렬');

INSERT INTO problem_revisions (
    problem_id, version, statement_ko, time_limit_ms, memory_limit_mb,
    checker_kind, content_hash
)
SELECT id, 1, statement_ko, time_limit_ms, memory_limit_mb, checker_kind,
       digest(statement_ko || E'\x1f' || time_limit_ms::text || E'\x1f' || memory_limit_mb::text || E'\x1f' || checker_kind, 'sha256')
FROM problems WHERE slug = 'docs-url-normalizer';

UPDATE problems problem
SET current_revision_id = revision.id,
    status = 'published',
    published_at = now()
FROM problem_revisions revision
WHERE problem.slug = 'docs-url-normalizer'
  AND revision.problem_id = problem.id
  AND revision.version = 1;

INSERT INTO problem_test_cases (problem_id, ordinal, input, expected_output, visibility, score_weight, group_key) VALUES
    ((SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 1, E'HTTPS://Example.COM/path?b=2&a=1#section\n', E'https://example.com/path?a=1&b=2\n', 'sample', 1, 'main'),
    ((SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 2, E'https://EXAMPLE.com/p?z=&a=2&a=1#frag\n', E'https://example.com/p?a=1&a=2&z=\n', 'hidden', 1, 'main'),
    ((SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 3, E'HTTP://Example.COM/%7Euser\n', E'http://example.com/%7Euser\n', 'hidden', 1, 'main');

CREATE TABLE lesson_problem_exercises (
    lesson_id uuid NOT NULL REFERENCES lessons(id) ON DELETE CASCADE,
    problem_id uuid NOT NULL REFERENCES problems(id) ON DELETE RESTRICT,
    exercise_kind text NOT NULL CHECK (exercise_kind IN ('exercise', 'checkpoint', 'capstone')),
    position integer NOT NULL CHECK (position > 0),
    PRIMARY KEY (lesson_id, problem_id),
    UNIQUE (lesson_id, position)
);

INSERT INTO lesson_problem_exercises (lesson_id, problem_id, exercise_kind, position) VALUES
    ((SELECT id FROM lessons WHERE slug = 'build-url-normalizer'), (SELECT id FROM problems WHERE slug = 'docs-url-normalizer'), 'capstone', 1);

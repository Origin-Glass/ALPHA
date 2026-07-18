CREATE TABLE class_invitations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    class_id uuid NOT NULL REFERENCES classes(id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    class_role text NOT NULL CHECK (class_role IN ('INSTRUCTOR', 'LEARNER')),
    expires_at timestamptz NOT NULL,
    accepted_by uuid REFERENCES users(id),
    accepted_at timestamptz,
    created_by uuid NOT NULL REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at),
    CHECK ((accepted_by IS NULL) = (accepted_at IS NULL))
);

CREATE INDEX class_invitations_active_idx
    ON class_invitations (expires_at) WHERE accepted_at IS NULL;

CREATE TABLE class_assignments (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    class_id uuid NOT NULL REFERENCES classes(id) ON DELETE CASCADE,
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    description_ko text NOT NULL CHECK (char_length(description_ko) BETWEEN 1 AND 2000),
    published_at timestamptz NOT NULL DEFAULT now(),
    due_at timestamptz NOT NULL,
    completion_goal_percent smallint NOT NULL DEFAULT 80 CHECK (completion_goal_percent BETWEEN 1 AND 100),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    created_by uuid NOT NULL REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (due_at > published_at)
);

CREATE INDEX class_assignments_class_due_idx
    ON class_assignments (class_id, due_at) WHERE status = 'active';

CREATE TABLE class_assignment_items (
    assignment_id uuid NOT NULL REFERENCES class_assignments(id) ON DELETE CASCADE,
    item_kind text NOT NULL CHECK (item_kind IN ('problem', 'activity')),
    problem_id uuid REFERENCES problems(id) ON DELETE RESTRICT,
    activity_id uuid REFERENCES learning_activities(id) ON DELETE RESTRICT,
    position integer NOT NULL CHECK (position > 0),
    PRIMARY KEY (assignment_id, position),
    CHECK (
        (item_kind = 'problem' AND problem_id IS NOT NULL AND activity_id IS NULL)
        OR (item_kind = 'activity' AND activity_id IS NOT NULL AND problem_id IS NULL)
    )
);

CREATE UNIQUE INDEX class_assignment_problem_once_idx
    ON class_assignment_items (assignment_id, problem_id) WHERE problem_id IS NOT NULL;
CREATE UNIQUE INDEX class_assignment_activity_once_idx
    ON class_assignment_items (assignment_id, activity_id) WHERE activity_id IS NOT NULL;

INSERT INTO users (handle, display_name, terms_accepted_at, terms_accepted_version)
VALUES ('alpha-demo-instructor', 'ALPHA 데모 강사', now(), '2026-07-18')
ON CONFLICT (handle) DO NOTHING;

INSERT INTO user_roles (user_id, role)
SELECT id, 'INSTRUCTOR' FROM users WHERE handle = 'alpha-demo-instructor'
ON CONFLICT DO NOTHING;

UPDATE profile_preferences
SET visibility = 'private', show_in_rankings = false, allow_friend_requests = false
WHERE user_id = (SELECT id FROM users WHERE handle = 'alpha-demo-instructor');

INSERT INTO organizations (slug, name, created_by)
SELECT 'alpha-demo-school', 'ALPHA 데모 학교', id FROM users WHERE handle = 'alpha-demo-instructor'
ON CONFLICT (slug) DO NOTHING;

INSERT INTO organization_memberships (organization_id, user_id, role)
SELECT organization.id, instructor.id, 'OWNER'
FROM organizations organization
JOIN users instructor ON instructor.handle = 'alpha-demo-instructor'
WHERE organization.slug = 'alpha-demo-school'
ON CONFLICT DO NOTHING;

INSERT INTO classes (organization_id, name, created_by)
SELECT organization.id, '알고리즘 기초반', instructor.id
FROM organizations organization
JOIN users instructor ON instructor.handle = 'alpha-demo-instructor'
WHERE organization.slug = 'alpha-demo-school'
  AND NOT EXISTS (
      SELECT 1 FROM classes class
      WHERE class.organization_id = organization.id AND class.name = '알고리즘 기초반'
  );

INSERT INTO class_memberships (class_id, user_id, role)
SELECT class.id, instructor.id, 'INSTRUCTOR'
FROM classes class
JOIN organizations organization ON organization.id = class.organization_id
JOIN users instructor ON instructor.handle = 'alpha-demo-instructor'
WHERE organization.slug = 'alpha-demo-school' AND class.name = '알고리즘 기초반'
ON CONFLICT DO NOTHING;

INSERT INTO class_assignments (
    class_id, title_ko, description_ko, due_at, completion_goal_percent, created_by
)
SELECT class.id, '첫 주: 읽고 구현하기',
       '코드를 읽어 상태를 추적한 뒤 두 수의 합 문제로 구현을 검증합니다.',
       now() + interval '30 days', 80, instructor.id
FROM classes class
JOIN organizations organization ON organization.id = class.organization_id
JOIN users instructor ON instructor.handle = 'alpha-demo-instructor'
WHERE organization.slug = 'alpha-demo-school' AND class.name = '알고리즘 기초반'
  AND NOT EXISTS (
      SELECT 1 FROM class_assignments assignment
      WHERE assignment.class_id = class.id AND assignment.title_ko = '첫 주: 읽고 구현하기'
  );

INSERT INTO class_assignment_items (assignment_id, item_kind, activity_id, position)
SELECT assignment.id, 'activity', activity.id, 1
FROM class_assignments assignment
JOIN classes class ON class.id = assignment.class_id
JOIN organizations organization ON organization.id = class.organization_id
JOIN learning_activities activity ON activity.slug = 'predict-nested-loop-output'
WHERE organization.slug = 'alpha-demo-school' AND assignment.title_ko = '첫 주: 읽고 구현하기'
ON CONFLICT DO NOTHING;

INSERT INTO class_assignment_items (assignment_id, item_kind, problem_id, position)
SELECT assignment.id, 'problem', problem.id, 2
FROM class_assignments assignment
JOIN classes class ON class.id = assignment.class_id
JOIN organizations organization ON organization.id = class.organization_id
JOIN problems problem ON problem.slug = 'alpha-pair-sum'
WHERE organization.slug = 'alpha-demo-school' AND assignment.title_ko = '첫 주: 읽고 구현하기'
ON CONFLICT DO NOTHING;

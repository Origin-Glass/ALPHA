CREATE TABLE progression_state (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    xp bigint NOT NULL DEFAULT 0 CHECK (xp >= 0),
    level integer NOT NULL DEFAULT 1 CHECK (level > 0),
    current_streak integer NOT NULL DEFAULT 0 CHECK (current_streak >= 0),
    best_streak integer NOT NULL DEFAULT 0 CHECK (best_streak >= current_streak),
    streak_protections integer NOT NULL DEFAULT 1 CHECK (streak_protections BETWEEN 0 AND 30),
    last_meaningful_date date,
    mastered_count integer NOT NULL DEFAULT 0 CHECK (mastered_count >= 0),
    independent_mastered_count integer NOT NULL DEFAULT 0 CHECK (
        independent_mastered_count BETWEEN 0 AND mastered_count
    ),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_mastery (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    axis text NOT NULL CHECK (axis IN (
        'algorithm', 'code_reading', 'debugging', 'documentation',
        'framework', 'contest', 'instructor_course'
    )),
    rating integer NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 1000),
    tier text NOT NULL DEFAULT '새싹' CHECK (tier IN ('새싹', '브론즈', '실버', '골드', '플래티넘', '마스터')),
    evidence_count integer NOT NULL DEFAULT 0 CHECK (evidence_count >= 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, axis)
);

CREATE TABLE seasons (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,47}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 100),
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('scheduled', 'active', 'closed')),
    CHECK (ends_at > starts_at)
);

CREATE TABLE xp_events (
    event_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    reward_key text NOT NULL CHECK (char_length(reward_key) BETWEEN 3 AND 200),
    source_kind text NOT NULL CHECK (source_kind IN (
        'activity', 'problem', 'contest', 'quest', 'course', 'admin_adjustment'
    )),
    source_id uuid,
    xp integer NOT NULL CHECK (xp BETWEEN -10000 AND 10000 AND xp <> 0),
    reason_ko text NOT NULL CHECK (char_length(reason_ko) BETWEEN 1 AND 300),
    season_id uuid REFERENCES seasons(id),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, reward_key)
);

CREATE INDEX xp_events_user_time_idx ON xp_events (user_id, occurred_at DESC);
CREATE INDEX xp_events_season_rank_idx ON xp_events (season_id, user_id) WHERE xp > 0;

CREATE TABLE mastery_events (
    event_id uuid NOT NULL REFERENCES xp_events(event_id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    axis text NOT NULL CHECK (axis IN (
        'algorithm', 'code_reading', 'debugging', 'documentation',
        'framework', 'contest', 'instructor_course'
    )),
    points integer NOT NULL CHECK (points BETWEEN 1 AND 100),
    mastery_class text NOT NULL CHECK (mastery_class IN (
        'completed', 'assisted', 'independent', 'contest_verified',
        'explanation_verified', 'review_verified'
    )),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, axis)
);

CREATE TABLE achievement_definitions (
    slug text PRIMARY KEY CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 100),
    description_ko text NOT NULL CHECK (char_length(description_ko) BETWEEN 1 AND 500),
    icon_token text NOT NULL CHECK (icon_token ~ '^[a-z0-9_-]+$'),
    active boolean NOT NULL DEFAULT true
);

CREATE TABLE user_achievements (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    achievement_slug text NOT NULL REFERENCES achievement_definitions(slug),
    evidence_event_id uuid NOT NULL REFERENCES xp_events(event_id),
    earned_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, achievement_slug)
);

CREATE TABLE quest_definitions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    title_ko text NOT NULL CHECK (char_length(title_ko) BETWEEN 1 AND 120),
    cadence text NOT NULL CHECK (cadence IN ('daily', 'weekly')),
    metric text NOT NULL CHECK (metric IN ('meaningful_completion', 'independent_mastery')),
    target integer NOT NULL CHECK (target BETWEEN 1 AND 100),
    reward_xp integer NOT NULL CHECK (reward_xp BETWEEN 1 AND 1000),
    active boolean NOT NULL DEFAULT true
);

CREATE TABLE user_quest_progress (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    quest_id uuid NOT NULL REFERENCES quest_definitions(id) ON DELETE CASCADE,
    period_start date NOT NULL,
    progress integer NOT NULL DEFAULT 0 CHECK (progress >= 0),
    completed_at timestamptz,
    reward_event_id uuid UNIQUE REFERENCES xp_events(event_id),
    PRIMARY KEY (user_id, quest_id, period_start)
);

CREATE TABLE title_definitions (
    slug text PRIMARY KEY CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    label_ko text NOT NULL CHECK (char_length(label_ko) BETWEEN 1 AND 60),
    unlock_kind text NOT NULL CHECK (unlock_kind IN ('default', 'achievement', 'season'))
);

CREATE TABLE cosmetic_definitions (
    slug text PRIMARY KEY CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    label_ko text NOT NULL CHECK (char_length(label_ko) BETWEEN 1 AND 60),
    css_token text NOT NULL UNIQUE CHECK (css_token ~ '^[a-z0-9_-]+$'),
    unlock_kind text NOT NULL CHECK (unlock_kind IN ('default', 'achievement', 'season'))
);

CREATE TABLE user_collectibles (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    collectible_kind text NOT NULL CHECK (collectible_kind IN ('title', 'cosmetic')),
    collectible_slug text NOT NULL CHECK (collectible_slug ~ '^[a-z0-9][a-z0-9-]{2,63}$'),
    granted_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, collectible_kind, collectible_slug)
);

CREATE TABLE profile_preferences (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    bio_ko text NOT NULL DEFAULT '' CHECK (char_length(bio_ko) <= 500),
    visibility text NOT NULL DEFAULT 'public' CHECK (visibility IN ('public', 'private')),
    show_in_rankings boolean NOT NULL DEFAULT true,
    allow_friend_requests boolean NOT NULL DEFAULT true,
    selected_title_slug text NOT NULL DEFAULT 'learner' REFERENCES title_definitions(slug),
    selected_cosmetic_slug text NOT NULL DEFAULT 'alpha-violet' REFERENCES cosmetic_definitions(slug),
    updated_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO seasons (slug, title_ko, starts_at, ends_at, status) VALUES
    ('launch-2026', 'ALPHA 시작 시즌', '2026-07-01T00:00:00+09:00', '2026-10-01T00:00:00+09:00', 'active');

INSERT INTO achievement_definitions (slug, title_ko, description_ko, icon_token) VALUES
    ('first-independent', '내 힘으로 첫 증명', '도움 없이 첫 활동을 통과했습니다.', 'spark'),
    ('three-independent', '독립 해결 세 걸음', '서로 다른 학습 항목 세 개를 도움 없이 통과했습니다.', 'steps'),
    ('seven-day-streak', '의미 있는 일주일', '보상 가능한 학습을 7일 연속 이어갔습니다.', 'flame'),
    ('docs-reader', '공식 문서 탐험가', '공식 문서 체크포인트를 통과했습니다.', 'book');

INSERT INTO quest_definitions (slug, title_ko, cadence, metric, target, reward_xp) VALUES
    ('daily-meaningful', '오늘의 의미 있는 학습', 'daily', 'meaningful_completion', 1, 20),
    ('weekly-independent-three', '이번 주 독립 해결 3회', 'weekly', 'independent_mastery', 3, 80);

INSERT INTO title_definitions (slug, label_ko, unlock_kind) VALUES
    ('learner', '배우는 사람', 'default'),
    ('independent-solver', '스스로 증명한 사람', 'achievement');

INSERT INTO cosmetic_definitions (slug, label_ko, css_token, unlock_kind) VALUES
    ('alpha-violet', '알파 바이올렛', 'alpha-violet', 'default'),
    ('independent-aurora', '독립 해결 오로라', 'independent-aurora', 'achievement');

INSERT INTO progression_state (user_id)
SELECT id FROM users ON CONFLICT DO NOTHING;

INSERT INTO user_mastery (user_id, axis)
SELECT id, axis
FROM users
CROSS JOIN (VALUES
    ('algorithm'), ('code_reading'), ('debugging'), ('documentation'),
    ('framework'), ('contest'), ('instructor_course')
) axes(axis)
ON CONFLICT DO NOTHING;

INSERT INTO profile_preferences (user_id, selected_title_slug, selected_cosmetic_slug)
SELECT id, 'learner', 'alpha-violet' FROM users ON CONFLICT DO NOTHING;

INSERT INTO user_collectibles (user_id, collectible_kind, collectible_slug)
SELECT id, kind, slug
FROM users
CROSS JOIN (VALUES ('title', 'learner'), ('cosmetic', 'alpha-violet')) defaults(kind, slug)
ON CONFLICT DO NOTHING;

CREATE FUNCTION initialize_user_progression() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO progression_state (user_id) VALUES (NEW.id);
    INSERT INTO user_mastery (user_id, axis) VALUES
        (NEW.id, 'algorithm'),
        (NEW.id, 'code_reading'),
        (NEW.id, 'debugging'),
        (NEW.id, 'documentation'),
        (NEW.id, 'framework'),
        (NEW.id, 'contest'),
        (NEW.id, 'instructor_course');
    INSERT INTO profile_preferences (user_id, selected_title_slug, selected_cosmetic_slug)
    VALUES (NEW.id, 'learner', 'alpha-violet');
    INSERT INTO user_collectibles (user_id, collectible_kind, collectible_slug) VALUES
        (NEW.id, 'title', 'learner'),
        (NEW.id, 'cosmetic', 'alpha-violet');
    RETURN NEW;
END;
$$;

CREATE TRIGGER users_initialize_progression
AFTER INSERT ON users
FOR EACH ROW EXECUTE FUNCTION initialize_user_progression();

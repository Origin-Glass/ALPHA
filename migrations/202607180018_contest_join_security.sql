CREATE TABLE contest_join_user_failures (
    contest_id uuid NOT NULL REFERENCES contests(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    failures integer NOT NULL CHECK (failures BETWEEN 1 AND 5),
    window_started_at timestamptz NOT NULL,
    locked_until timestamptz,
    PRIMARY KEY (contest_id, user_id)
);

CREATE TABLE contest_join_global_failures (
    contest_id uuid PRIMARY KEY REFERENCES contests(id) ON DELETE CASCADE,
    failures integer NOT NULL CHECK (failures BETWEEN 1 AND 60),
    window_started_at timestamptz NOT NULL,
    locked_until timestamptz
);

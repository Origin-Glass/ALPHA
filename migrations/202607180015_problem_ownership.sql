ALTER TABLE problems
    ADD COLUMN created_by uuid REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX problems_created_by_idx
    ON problems (created_by)
    WHERE created_by IS NOT NULL;

ALTER TABLE submissions
    ADD COLUMN run_kind text NOT NULL DEFAULT 'formal'
        CHECK (run_kind IN ('formal', 'sample', 'custom')),
    ADD COLUMN custom_input text CHECK (octet_length(custom_input) <= 65536),
    ADD COLUMN run_output text CHECK (octet_length(run_output) <= 1048576),
    ADD CHECK ((run_kind = 'custom') = (custom_input IS NOT NULL));

CREATE INDEX submissions_formal_history_idx
    ON submissions (user_id, problem_id, created_at DESC)
    WHERE run_kind = 'formal';

ALTER TABLE problem_test_cases
    ADD COLUMN problem_revision_id uuid REFERENCES problem_revisions(id);

UPDATE problem_test_cases test_case
SET problem_revision_id = problem.current_revision_id
FROM problems problem
WHERE problem.id = test_case.problem_id;

ALTER TABLE problem_test_cases
    ALTER COLUMN problem_revision_id SET NOT NULL,
    DROP CONSTRAINT problem_test_cases_problem_id_ordinal_key,
    ADD CONSTRAINT problem_test_cases_revision_ordinal_key
        UNIQUE (problem_revision_id, ordinal);

CREATE INDEX problem_test_cases_revision_judge_idx
    ON problem_test_cases (problem_revision_id, ordinal);

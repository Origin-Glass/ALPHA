ALTER TABLE problem_revisions
    ADD CONSTRAINT problem_revisions_id_problem_unique UNIQUE (id, problem_id);

ALTER TABLE problem_test_cases
    DROP CONSTRAINT problem_test_cases_problem_revision_id_fkey,
    ADD CONSTRAINT problem_test_cases_revision_problem_fkey
        FOREIGN KEY (problem_revision_id, problem_id)
        REFERENCES problem_revisions (id, problem_id)
        ON DELETE CASCADE;

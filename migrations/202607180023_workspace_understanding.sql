CREATE TABLE workspace_templates (
    id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision > 0),
    slug text NOT NULL CHECK (slug ~ '^[a-z0-9-]{3,80}$'),
    track_kind text NOT NULL CHECK (track_kind IN ('cli_library','backend_api','frontend_interactive')),
    template_digest bytea NOT NULL CHECK (octet_length(template_digest)=32),
    image_reference text NOT NULL CHECK (image_reference ~ '@sha256:[0-9a-f]{64}$'),
    run_command jsonb NOT NULL CHECK (jsonb_typeof(run_command)='array' AND jsonb_array_length(run_command) BETWEEN 1 AND 16),
    check_suite_id text NOT NULL CHECK (check_suite_id ~ '^[a-z0-9.-]{3,80}$'),
    check_suite_hash bytea NOT NULL CHECK (octet_length(check_suite_hash)=32),
    supports_tests boolean NOT NULL DEFAULT false,
    network_policy text NOT NULL DEFAULT 'none' CHECK (network_policy='none'),
    background_services jsonb NOT NULL DEFAULT '[]' CHECK (background_services IN ('[]','[{"kind":"loopback_http_test","version":"v1"}]')),
    dependency_cache_status text NOT NULL DEFAULT 'not_required' CHECK(dependency_cache_status='not_required'),
    cache_digest bytea CHECK(cache_digest IS NULL),
    runtime_status text NOT NULL CHECK (runtime_status IN ('verified','implemented_unverified','blocked')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (id, revision), UNIQUE (slug, revision), UNIQUE (id, revision, template_digest)
);

INSERT INTO workspace_templates (id,revision,slug,track_kind,template_digest,image_reference,run_command,check_suite_id,check_suite_hash,supports_tests,runtime_status) VALUES
('23000000-0000-7000-8000-000000000001',1,'rust-cli-v1','cli_library',decode(repeat('11',32),'hex'),'alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de','["sh","-lc","rustc main.rs -o /tmp/app && /tmp/app"]','rust-cli-build-run-v1',decode(repeat('a1',32),'hex'),false,'implemented_unverified'),
('23000000-0000-7000-8000-000000000004',1,'python-cli-v1','cli_library',decode(repeat('44',32),'hex'),'alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de','["python3","main.py"]','python-cli-run-v1',decode(repeat('d4',32),'hex'),false,'verified'),
('23000000-0000-7000-8000-000000000005',1,'python-test-v1','cli_library',decode(repeat('55',32),'hex'),'alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de','["sh","-lc","PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v"]','python-unittest-v1',decode(repeat('e5',32),'hex'),true,'verified'),
('23000000-0000-7000-8000-000000000002',1,'python-api-v1','backend_api',decode(repeat('22',32),'hex'),'alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de','["sh","-lc","PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v"]','python-api-loopback-v1',decode(repeat('b2',32),'hex'),true,'verified'),
('23000000-0000-7000-8000-000000000003',1,'static-web-v1','frontend_interactive',decode(repeat('33',32),'hex'),'alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de','["python3","-c","s=open(''index.html'',encoding=''utf-8'').read().lower();assert ''<html'' in s and ''<body'' in s and ''</body>'' in s"]','static-web-structure-v1',decode(repeat('c3',32),'hex'),false,'verified');
UPDATE workspace_templates SET background_services='[{"kind":"loopback_http_test","version":"v1"}]' WHERE slug='python-api-v1';

CREATE TABLE project_workspaces (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id uuid,
    template_id uuid NOT NULL,
    template_revision integer NOT NULL,
    template_digest bytea NOT NULL CHECK (octet_length(template_digest)=32),
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    version integer NOT NULL DEFAULT 1 CHECK (version > 0),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash)=32),
    idempotency_key uuid NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK(status IN('active','expired')),
    expires_at timestamptz NOT NULL DEFAULT now()+interval '7 days',
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id,user_id), UNIQUE (user_id,idempotency_key),
    FOREIGN KEY (project_id,user_id) REFERENCES learner_projects(id,user_id) ON DELETE CASCADE,
    FOREIGN KEY (template_id,template_revision,template_digest) REFERENCES workspace_templates(id,revision,template_digest) ON DELETE RESTRICT
);

CREATE TABLE workspace_revisions (
    workspace_id uuid NOT NULL,user_id uuid NOT NULL,version integer NOT NULL,
    artifact jsonb NOT NULL CHECK(jsonb_typeof(artifact)='array'),artifact_hash bytea NOT NULL CHECK(octet_length(artifact_hash)=32),
    created_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(workspace_id,version),UNIQUE(workspace_id,user_id,version),
    FOREIGN KEY(workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE
);
CREATE TABLE workspace_checkpoints (
    id uuid PRIMARY KEY,workspace_id uuid NOT NULL,user_id uuid NOT NULL,version integer NOT NULL,
    name text NOT NULL CHECK(char_length(name) BETWEEN 1 AND 80),idempotency_key uuid NOT NULL,request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32),created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(user_id,idempotency_key),FOREIGN KEY(workspace_id,user_id,version) REFERENCES workspace_revisions(workspace_id,user_id,version) ON DELETE RESTRICT
);

CREATE TABLE workspace_mutations (
    workspace_id uuid NOT NULL, user_id uuid NOT NULL, idempotency_key uuid NOT NULL,
    request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32), resulting_version integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(user_id,idempotency_key),
    FOREIGN KEY(workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE
);

CREATE TABLE workspace_files (
    workspace_id uuid NOT NULL, user_id uuid NOT NULL, path text NOT NULL,
    path_key text NOT NULL, content text NOT NULL,
    content_hash bytea NOT NULL CHECK (octet_length(content_hash)=32),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id,path), UNIQUE(workspace_id,path_key),
    FOREIGN KEY (workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE,
    CHECK (path !~ '(^/|(^|/)\.\.(/|$)|[[:cntrl:]])'),
    CHECK (octet_length(path) BETWEEN 1 AND 160), CHECK (octet_length(content) <= 262144)
);

CREATE TABLE workspace_runs (
    id uuid PRIMARY KEY, workspace_id uuid NOT NULL, user_id uuid NOT NULL,
    workspace_version integer NOT NULL CHECK (workspace_version > 0),
    template_id uuid NOT NULL, template_revision integer NOT NULL,
    template_digest bytea NOT NULL CHECK (octet_length(template_digest)=32),
    image_reference text NOT NULL CHECK (image_reference ~ '@sha256:[0-9a-f]{64}$'),
    execution_image_digest text CHECK (execution_image_digest IS NULL OR execution_image_digest ~ '^sha256:[0-9a-f]{64}$'),
    artifact jsonb NOT NULL CHECK (jsonb_typeof(artifact)='array'),
    artifact_hash bytea NOT NULL CHECK (octet_length(artifact_hash)=32),
    semantic_hash bytea NOT NULL CHECK (octet_length(semantic_hash)=32),
    validation_kind text NOT NULL DEFAULT 'baseline' CHECK(validation_kind IN('baseline','modification','transfer')),
    challenge_id uuid,
    check_suite_id text NOT NULL, check_suite_hash bytea NOT NULL CHECK(octet_length(check_suite_hash)=32),
    supports_tests_snapshot boolean NOT NULL,
    background_services_snapshot jsonb NOT NULL CHECK(jsonb_typeof(background_services_snapshot)='array'),
    dependency_cache_status text NOT NULL CHECK(dependency_cache_status='not_required'),
    cache_digest bytea CHECK(cache_digest IS NULL),
    command jsonb NOT NULL CHECK (jsonb_typeof(command)='array' AND jsonb_array_length(command) BETWEEN 1 AND 16),
    status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','leased','running','succeeded','failed','cancelled','expired')),
    attempt smallint NOT NULL DEFAULT 0 CHECK (attempt BETWEEN 0 AND 3),
    lease_token uuid, leased_by text, lease_expires_at timestamptz, cancel_requested_at timestamptz,
    cancel_idempotency_key uuid, cancel_request_hash bytea CHECK(cancel_request_hash IS NULL OR octet_length(cancel_request_hash)=32),
    exit_code integer, stdout text, stderr text,
    stdout_hash bytea CHECK(stdout_hash IS NULL OR octet_length(stdout_hash)=32),
    deterministic_checks_passed boolean NOT NULL DEFAULT false,
    runner_receipt_hash bytea CHECK(runner_receipt_hash IS NULL OR octet_length(runner_receipt_hash)=32),
    runner_receipt_mac bytea CHECK(runner_receipt_mac IS NULL OR octet_length(runner_receipt_mac)=32),
    output_truncated boolean NOT NULL DEFAULT false,
    idempotency_key uuid NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), completed_at timestamptz,
    request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32),
    UNIQUE (id,user_id), UNIQUE(user_id,idempotency_key),
    FOREIGN KEY (workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE,
    FOREIGN KEY (template_id,template_revision,template_digest) REFERENCES workspace_templates(id,revision,template_digest) ON DELETE RESTRICT,
    CHECK ((status IN ('leased','running')) = (lease_token IS NOT NULL AND leased_by IS NOT NULL AND lease_expires_at IS NOT NULL))
);
CREATE INDEX workspace_runs_queue_idx ON workspace_runs(created_at) WHERE status='queued';

CREATE TABLE understanding_challenges (
    id uuid PRIMARY KEY, user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL, source_run_id uuid NOT NULL,
    artifact_hash bytea NOT NULL CHECK(octet_length(artifact_hash)=32),
    source_semantic_hash bytea NOT NULL CHECK(octet_length(source_semantic_hash)=32),
    source_stdout_hash bytea NOT NULL CHECK(octet_length(source_stdout_hash)=32),
    source_stdout text NOT NULL,
    check_suite_id text NOT NULL, check_suite_hash bytea NOT NULL CHECK(octet_length(check_suite_hash)=32),
    source_template_digest bytea NOT NULL CHECK(octet_length(source_template_digest)=32),
    source_track_kind text NOT NULL CHECK(source_track_kind IN('cli_library','backend_api','frontend_interactive')),
    source_facts jsonb NOT NULL CHECK(jsonb_typeof(source_facts)='object'),
    kind text NOT NULL CHECK(kind IN ('explanation_modification_transfer')),
    prompt jsonb NOT NULL CHECK(jsonb_typeof(prompt)='object'),
    expected_concepts jsonb NOT NULL CHECK(jsonb_typeof(expected_concepts)='array' AND jsonb_array_length(expected_concepts)>0),
    modification_prediction text, transfer_prediction text, predictions_committed_at timestamptz,
    prediction_idempotency_key uuid, prediction_request_hash bytea CHECK(prediction_request_hash IS NULL OR octet_length(prediction_request_hash)=32),
    state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','reviewed','understood','independently_modifiable','transfer_verified')),
    idempotency_key uuid NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32),
    UNIQUE(id,user_id), UNIQUE(user_id,idempotency_key),
    FOREIGN KEY(workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE,
    FOREIGN KEY(source_run_id,user_id) REFERENCES workspace_runs(id,user_id) ON DELETE RESTRICT
);

ALTER TABLE workspace_runs ADD FOREIGN KEY(challenge_id,user_id) REFERENCES understanding_challenges(id,user_id) ON DELETE RESTRICT;
CREATE UNIQUE INDEX understanding_one_pending_source_idx ON understanding_challenges(user_id,workspace_id,source_run_id) WHERE state='pending';

CREATE TABLE understanding_receipts (
    id uuid PRIMARY KEY, challenge_id uuid NOT NULL, user_id uuid NOT NULL,
    source_artifact_hash bytea NOT NULL CHECK(octet_length(source_artifact_hash)=32),
    explanation text NOT NULL CHECK(char_length(explanation) BETWEEN 20 AND 4000),
    modification_run_id uuid NOT NULL, modification_hash bytea NOT NULL CHECK(octet_length(modification_hash)=32),
    transfer_run_id uuid NOT NULL, transfer_hash bytea NOT NULL CHECK(octet_length(transfer_hash)=32),
    deterministic_checks_passed boolean NOT NULL, ai_assessed boolean NOT NULL DEFAULT false,
    state text NOT NULL CHECK(state IN ('transfer_verified')),
    receipt_hash bytea NOT NULL CHECK(octet_length(receipt_hash)=32), idempotency_key uuid NOT NULL,
    request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(challenge_id), UNIQUE(user_id,idempotency_key),
    FOREIGN KEY(challenge_id,user_id) REFERENCES understanding_challenges(id,user_id) ON DELETE RESTRICT,
    FOREIGN KEY(modification_run_id,user_id) REFERENCES workspace_runs(id,user_id) ON DELETE RESTRICT,
    FOREIGN KEY(transfer_run_id,user_id) REFERENCES workspace_runs(id,user_id) ON DELETE RESTRICT,
    CHECK(ai_assessed=false), CHECK(modification_hash<>source_artifact_hash), CHECK(transfer_hash<>source_artifact_hash), CHECK(transfer_hash<>modification_hash)
);

CREATE TABLE portfolio_rights_receipts (
    id uuid PRIMARY KEY, workspace_id uuid NOT NULL, user_id uuid NOT NULL,
    workspace_version integer NOT NULL, artifact_hash bytea NOT NULL CHECK(octet_length(artifact_hash)=32),
    provenance text NOT NULL CHECK(provenance IN ('original','licensed','authorized_import')),
    license_identifier text NOT NULL CHECK(char_length(license_identifier) BETWEEN 2 AND 120),
    decision text NOT NULL CHECK(decision='approved'), reviewed_by uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    receipt_hash bytea NOT NULL CHECK(octet_length(receipt_hash)=32), idempotency_key uuid NOT NULL,
    request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32), created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(id,user_id), UNIQUE(workspace_id,user_id,workspace_version,artifact_hash),
    UNIQUE(reviewed_by,idempotency_key),
    FOREIGN KEY(workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE,
    CHECK(reviewed_by<>user_id)
);

CREATE TABLE portfolio_items (
    id uuid PRIMARY KEY, user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL, source_run_id uuid NOT NULL, rights_receipt_id uuid NOT NULL,
    title text NOT NULL CHECK(char_length(title) BETWEEN 1 AND 120),
    problem text NOT NULL CHECK(char_length(problem) BETWEEN 1 AND 1000), target_user text NOT NULL CHECK(char_length(target_user) BETWEEN 1 AND 300),
    visibility text NOT NULL CHECK(visibility IN ('private','public')),
    assistance_disclosure text NOT NULL CHECK(char_length(assistance_disclosure) BETWEEN 2 AND 2000),
    evidence_labels jsonb NOT NULL CHECK(jsonb_typeof(evidence_labels)='array' AND jsonb_array_length(evidence_labels)>0),
    status text NOT NULL DEFAULT 'draft' CHECK(status IN ('draft','published','withdrawn')),
    idempotency_key uuid NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), published_at timestamptz,
    request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32),
    UNIQUE(id,user_id), UNIQUE(user_id,idempotency_key),
    FOREIGN KEY(workspace_id,user_id) REFERENCES project_workspaces(id,user_id) ON DELETE CASCADE,
    FOREIGN KEY(source_run_id,user_id) REFERENCES workspace_runs(id,user_id) ON DELETE RESTRICT,
    FOREIGN KEY(rights_receipt_id,user_id) REFERENCES portfolio_rights_receipts(id,user_id) ON DELETE RESTRICT
);

CREATE FUNCTION deny_workspace_evidence_mutation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'immutable workspace evidence'; END $$;
CREATE TRIGGER workspace_runs_no_update AFTER UPDATE ON workspace_runs FOR EACH ROW WHEN (OLD.status IN ('succeeded','failed','cancelled','expired')) EXECUTE FUNCTION deny_workspace_evidence_mutation();
CREATE TRIGGER workspace_runs_terminal_no_delete BEFORE DELETE ON workspace_runs FOR EACH ROW WHEN (OLD.status IN ('succeeded','failed','cancelled','expired')) EXECUTE FUNCTION deny_workspace_evidence_mutation();
CREATE TRIGGER workspace_templates_no_mutation BEFORE UPDATE OR DELETE ON workspace_templates FOR EACH ROW EXECUTE FUNCTION deny_workspace_evidence_mutation();
CREATE TRIGGER workspace_revisions_no_mutation BEFORE UPDATE OR DELETE ON workspace_revisions FOR EACH ROW EXECUTE FUNCTION deny_workspace_evidence_mutation();
CREATE TRIGGER understanding_receipts_no_update BEFORE UPDATE OR DELETE ON understanding_receipts FOR EACH ROW EXECUTE FUNCTION deny_workspace_evidence_mutation();
CREATE TRIGGER portfolio_rights_no_update BEFORE UPDATE OR DELETE ON portfolio_rights_receipts FOR EACH ROW EXECUTE FUNCTION deny_workspace_evidence_mutation();

CREATE FUNCTION enforce_workspace_run_transition() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF (NEW.workspace_id,NEW.user_id,NEW.workspace_version,NEW.template_id,NEW.template_revision,NEW.template_digest,NEW.image_reference,NEW.artifact,NEW.artifact_hash,NEW.semantic_hash,NEW.validation_kind,NEW.challenge_id,NEW.check_suite_id,NEW.check_suite_hash,NEW.supports_tests_snapshot,NEW.background_services_snapshot,NEW.dependency_cache_status,NEW.cache_digest,NEW.command,NEW.idempotency_key,NEW.request_hash)
     IS DISTINCT FROM
     (OLD.workspace_id,OLD.user_id,OLD.workspace_version,OLD.template_id,OLD.template_revision,OLD.template_digest,OLD.image_reference,OLD.artifact,OLD.artifact_hash,OLD.semantic_hash,OLD.validation_kind,OLD.challenge_id,OLD.check_suite_id,OLD.check_suite_hash,OLD.supports_tests_snapshot,OLD.background_services_snapshot,OLD.dependency_cache_status,OLD.cache_digest,OLD.command,OLD.idempotency_key,OLD.request_hash)
  THEN RAISE EXCEPTION 'workspace execution snapshot is immutable';
  END IF;
  IF NEW.status IN ('succeeded','failed') AND NOT (
    OLD.status='running' AND OLD.lease_token IS NOT NULL AND
    NEW.runner_receipt_hash IS NOT NULL AND NEW.runner_receipt_mac IS NOT NULL AND
    NEW.execution_image_digest IS NOT NULL
  ) THEN RAISE EXCEPTION 'terminal workspace result requires active worker lease and signed receipt';
  END IF;
  IF OLD.status='queued' AND NEW.status NOT IN ('queued','leased','cancelled','expired') THEN
    RAISE EXCEPTION 'invalid workspace run transition';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER workspace_run_transition BEFORE UPDATE ON workspace_runs FOR EACH ROW EXECUTE FUNCTION enforce_workspace_run_transition();

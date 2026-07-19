\set ON_ERROR_STOP on

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM users) THEN
    RAISE EXCEPTION 'recovery fixture requires an existing user';
  END IF;
END $$;

INSERT INTO content_provider_configs
  (id,name,kind,protocol,base_url,model,cost_per_generation_microunits,created_by)
SELECT '60000000-0000-7000-8000-000000000001','recovery-fixture','local',
       'openai_compatible','http://127.0.0.1:11434','recovery-model',1,id
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_generation_jobs
  (id,created_by,provider_id,content_type,request_spec,request_hash,status,
   estimated_cost_microunits,attempt_cost_microunits,max_attempts,settled,attempt_count,
   created_at,updated_at)
SELECT '60000000-0000-7000-8000-000000000002',id,
       '60000000-0000-7000-8000-000000000001','algorithm_problem',
       '{"fixture":"recovery-v1","topic":"canonical backup"}',decode(repeat('12',32),'hex'),
       'completed',3,1,3,true,1,'2026-07-19 00:00:00+00','2026-07-19 00:00:00+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_generation_attempts
  (id,job_id,attempt_number,provider_snapshot,request_hash,response_hash,status,
   calls_started,outputs_completed,usage,started_at,completed_at)
VALUES
  ('60000000-0000-7000-8000-000000000003','60000000-0000-7000-8000-000000000002',1,
   '{"model":"recovery-model","fixture":"recovery-v1"}',decode(repeat('12',32),'hex'),
   decode(repeat('23',32),'hex'),'completed',1,1,'{"input_tokens":7,"output_tokens":11}',
   '2026-07-19 00:00:01+00','2026-07-19 00:00:02+00');

INSERT INTO content_artifacts
  (id,job_id,attempt_id,artifact_kind,payload,content_hash,created_at)
VALUES
  ('60000000-0000-7000-8000-000000000004','60000000-0000-7000-8000-000000000002',
   '60000000-0000-7000-8000-000000000003','candidate',
   '{"fixture":"recovery-v1","title":"Canonical recovery artifact"}',decode(repeat('34',32),'hex'),
   '2026-07-19 00:00:03+00');

INSERT INTO content_review_items
  (id,artifact_id,artifact_hash,author_user_id,state,impact,create_idempotency_key,created_at,updated_at)
SELECT '60000000-0000-7000-8000-000000000005','60000000-0000-7000-8000-000000000004',
       decode(repeat('34',32),'hex'),id,'approved','standard',
       '60000000-0000-7000-8000-000000000010','2026-07-19 00:00:04+00','2026-07-19 00:00:04+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_review_revisions
  (review_item_id,revision_number,artifact_id,artifact_hash,changed_by,idempotency_key,request_hash,created_at)
SELECT '60000000-0000-7000-8000-000000000005',1,
       '60000000-0000-7000-8000-000000000004',decode(repeat('34',32),'hex'),id,
       '60000000-0000-7000-8000-000000000011',decode(repeat('45',32),'hex'),'2026-07-19 00:00:05+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_provenance_records
  (id,review_item_id,revision_number,artifact_hash,origin_type,creator_or_provider,
   source_revision,license_basis,license_identifier,attribution,modification_status,
   commercial_use_allowed,redistribution_allowed,ai_provider_id,ai_provider_name,ai_model,
   generation_job_id,generation_attempt_id,generation_run_reference,evidence_reference,
   evidence_hash,attachment_metadata,legal_status,provenance_hash,recorded_by,idempotency_key,created_at)
SELECT '60000000-0000-7000-8000-000000000006','60000000-0000-7000-8000-000000000005',1,
       decode(repeat('34',32),'hex'),'ai_generated','recovery-fixture','revision-1','provider_contract',
       'fixture-terms-v1','ALPHA recovery fixture','generated',true,true,
       '60000000-0000-7000-8000-000000000001','recovery-fixture','recovery-model',
       '60000000-0000-7000-8000-000000000002','60000000-0000-7000-8000-000000000003',
       'recovery-run-1','fixture evidence',decode(repeat('56',32),'hex'),'{"fixture":"recovery-v1"}',
       'pending',decode(repeat('67',32),'hex'),id,'60000000-0000-7000-8000-000000000012',
       '2026-07-19 00:00:06+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_ai_review_receipts
  (id,review_item_id,revision_number,review_kind,operator_user_id,provider,model,prompt_version,
   role_identifier,prompt_hash,review_seed,review_context,context_hash,review_response,input_hash,
   output_hash,latency_ms,usage,outcome,findings,idempotency_key,receipt_hash,created_at)
SELECT '60000000-0000-7000-8000-000000000007','60000000-0000-7000-8000-000000000005',1,
       'specification_pedagogy',id,'recovery-fixture','recovery-model','fixture-prompt-v1','fixture-reviewer',
       decode(repeat('78',32),'hex'),19,'{"fixture":"recovery-v1"}',decode(repeat('89',32),'hex'),
       '{"verdict":"pass","fixture":"recovery-v1"}',decode(repeat('34',32),'hex'),
       decode(repeat('9a',32),'hex'),17,'{"input_tokens":7,"output_tokens":11}','pass','[]',
       '60000000-0000-7000-8000-000000000013',decode(repeat('ab',32),'hex'),'2026-07-19 00:00:07+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_human_review_decisions
  (id,review_item_id,revision_number,reviewer_user_id,decision,note,idempotency_key,receipt_hash,created_at)
SELECT '60000000-0000-7000-8000-000000000008','60000000-0000-7000-8000-000000000005',1,id,
       'approve','recovery fixture approved','60000000-0000-7000-8000-000000000014',
       decode(repeat('bc',32),'hex'),'2026-07-19 00:00:08+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_rights_review_receipts
  (id,review_item_id,revision_number,reviewer_user_id,provenance_id,provenance_hash,decision,basis,
   evidence,commercial_use_allowed,redistribution_allowed,provider_terms_version,idempotency_key,
   receipt_hash,created_at)
SELECT '60000000-0000-7000-8000-000000000009','60000000-0000-7000-8000-000000000005',1,id,
       '60000000-0000-7000-8000-000000000006',decode(repeat('67',32),'hex'),'approve','contract',
       'recovery fixture rights approved',true,true,'fixture-terms-v1',
       '60000000-0000-7000-8000-000000000015',decode(repeat('cd',32),'hex'),'2026-07-19 00:00:09+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO content_pilot_receipts
  (id,review_item_id,revision_number,reviewer_user_id,cohort,source_reference,started_at,ended_at,
   participants,completion_rate,failure_rate,report_count,rollback_ready,rollback_evidence,decision,note,
   idempotency_key,receipt_hash,evidence_hash,created_at)
SELECT '60000000-0000-7000-8000-00000000000a','60000000-0000-7000-8000-000000000005',1,id,
       'recovery-fixture-cohort','recovery-fixture-report','2026-07-18 22:00:00+00','2026-07-18 23:00:00+00',
       5,1,0,0,true,'rollback fixture ready','pass','recovery fixture pilot passed',
       '60000000-0000-7000-8000-000000000016',decode(repeat('de',32),'hex'),
       decode(repeat('ef',32),'hex'),'2026-07-19 00:00:10+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO project_ideas
  (id,user_id,revision,idempotency_key,title,motivation,target_user,intended_outcome,core_feature,
   technology,weekly_minutes,assistance_policy,skill_level,runtime,infrastructure,requested_features,
   scoped_features,milestones,feasibility_reasons,excluded_features,scope_reduced,lineage_id,input,input_hash,
   rule_version,created_at)
SELECT '60000000-0000-7000-8000-000000000020',id,1,
       '60000000-0000-7000-8000-000000000021','Recovery fixture project',
       'canonical recovery integrity','recovery operator','verified semantic restore','digest verifier',
       'python',120,'guided_ai','beginner','cli','local_only','["digest verifier"]',
       '["digest verifier"]','[{"title":"verify restore"}]','["small deterministic fixture"]','[]',
       false,'60000000-0000-7000-8000-000000000022','{"fixture":"recovery-v1"}',
       decode(repeat('f1',32),'hex'),'project-learning-v1','2026-07-19 00:00:11+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO policy_versions (version,title_ko,body_ko,required,published_at)
VALUES ('recovery-fixture-v1','복구 검증 정책',
        repeat('복구 검증용 고정 정책 본문입니다. 백업과 복원 과정에서 동의 내용과 버전이 정확히 보존되는지 확인합니다. ',4),
        true,'2026-07-19 00:00:12+00');

INSERT INTO policy_consents
  (user_id,policy_version,policy_title_ko,policy_body_ko,choices,consented_at)
SELECT id,'recovery-fixture-v1','복구 검증 정책',
       repeat('복구 검증용 고정 정책 본문입니다. 백업과 복원 과정에서 동의 내용과 버전이 정확히 보존되는지 확인합니다. ',4),
       '{"terms":true,"privacy":true,"fixture":"recovery-v1"}',
       '2026-07-19 00:00:13+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO project_workspaces
  (id,user_id,project_id,template_id,template_revision,template_digest,title,version,
   request_hash,idempotency_key,status,expires_at,created_at,updated_at)
SELECT '60000000-0000-7000-8000-000000000030',id,NULL,
       '23000000-0000-7000-8000-000000000004',1,decode(repeat('44',32),'hex'),
       'Recovery fixture workspace',1,decode(repeat('a1',32),'hex'),
       '60000000-0000-7000-8000-000000000031','active','2026-07-26 00:00:00+00',
       '2026-07-19 00:00:14+00','2026-07-19 00:00:14+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO workspace_revisions
  (workspace_id,user_id,version,artifact,artifact_hash,created_at)
SELECT '60000000-0000-7000-8000-000000000030',id,1,
       '[{"path":"main.py","content":"print(42)\\n"}]',decode(repeat('b2',32),'hex'),
       '2026-07-19 00:00:15+00'
FROM users ORDER BY id LIMIT 1;

INSERT INTO workspace_files
  (workspace_id,user_id,path,path_key,content,content_hash,updated_at)
SELECT '60000000-0000-7000-8000-000000000030',id,'main.py','main.py','print(42)\n',
       decode(repeat('c3',32),'hex'),'2026-07-19 00:00:16+00'
FROM users ORDER BY id LIMIT 1;

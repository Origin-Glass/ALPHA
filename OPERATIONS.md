# ALPHA 운영 runbook

## 배포 전 확인

1. 현재 `compose.production.yaml`은 Docker socket을 가진 판정·작업공간 작업자도 함께 실행하는 단일 호스트 구성임을 확인합니다. 별도 host override가 실제로 추가·검증되기 전에는 API·웹과 격리됐다고 기록하지 않습니다.
2. `.env.production.example`을 복사해 모든 필수값을 채우고 비밀값을 저장소·shell history에 남기지 않습니다.
3. API·웹·작업자·실행 이미지를 레지스트리의 `repository@sha256:64자리` 참조로 고정합니다.
4. 다음 구성이 성공하는지 확인합니다.

```bash
docker compose -f compose.production.yaml config --quiet
ruby ops/check-openapi.rb
```

`docker compose -f compose.production.yaml up -d`는 db, migrate, publication-gate, API/worker, web 순서로 기동합니다. `publication-gate`가 종료 코드 0이 아니면 뒤 서비스는 시작되지 않습니다.

## 게시 게이트

배포 전 또는 권리 검토 변경 뒤 직접 실행합니다.

```bash
docker compose -f compose.production.yaml run --rm publication-gate
```

`run --rm`은 종료 코드를 반환한 뒤 컨테이너를 삭제하므로 같은 컨테이너를 `ps`로 확인하지 않습니다. 감사 기록에는 명령의 종료 코드와 stderr를 보관합니다.

- 종료 코드 0: 현재 게시 후보의 상업 이용·재배포 권리 근거가 승인됨
- 종료 코드 1: stderr의 review ID와 `missing_provenance`, `missing_rights_approval`, `rights_not_approved`, `commercial_use_denied`, `redistribution_denied`, `stale_revision_evidence`를 관리자 검토 화면에서 해소해야 함
- 종료 코드 2: `DATABASE_URL`, 연결, 권한 또는 스키마 오류. 권리 상태를 읽지 못했으므로 배포 금지

DB 행을 직접 승인 상태로 바꾸지 않습니다. 현재 리비전에 provenance와 rights receipt를 기록한 뒤 게이트를 다시 실행합니다.

## 백업

배포·마이그레이션 직전과 정기 일정에 실행하고 dump와 `.sha256`을 함께 접근 제한 저장소로 옮깁니다.

```bash
export COMPOSE_FILE=compose.production.yaml
source_database=alpha
docker compose stop api metadata-worker content-worker judge-worker workspace-worker
backup_file=$(./ops/backup.sh /srv/alpha/backups "$source_database")
printf '%s\n' "$backup_file"
```

먼저 API와 모든 writer를 중지하고 실제 `DATABASE_URL`의 database 이름을 `source_database`에 명시합니다. 백업 대상 디렉터리는 운영 백업 계정이 소유하고 다른 계정이 쓸 수 없어야 합니다. `backup.sh`는 `umask 077`로 dump와 부속 파일을 소유자 전용으로 만들고, dump 전 canonical digest, custom dump, dump 후 canonical digest 순으로 수행하며 원본 digest가 달라지면 staging 결과를 삭제하고 실패합니다. 성공 시 dump `.sha256`, backup-time four-dataset `.semantic.json`, manifest `.sha256`을 dump와 같은 filesystem에서 원자적으로 게시합니다. 네 파일을 한 세트로 이동·보관합니다. 성공 로그와 source database, 보관 위치, 실행 시각, 운영자를 변경 기록에 남깁니다.

## 복구 훈련과 전환

복구 훈련은 운영 DB를 덮어쓰지 않고 임시 새 DB에서 수행합니다.

```bash
export COMPOSE_FILE=compose.production.yaml
source_database=alpha
backup_file=/srv/alpha/backups/alpha-YYYYMMDDTHHMMSSZ-PID.dump
restored_database=alpha_recovered_YYYYMMDD_unique
./ops/restore.sh "$backup_file" "$restored_database"
sh ./ops/recovery-smoke.sh "$backup_file" "$restored_database" "$source_database"
```

성공하려면 생성 작업·attempt·artifact, AI/사람/권리/pilot 검토 영수증, 정책 버전·동의, 프로젝트 입력·작업공간 revision/file hash가 backup-time manifest의 4개 digest와 정확히 같아야 합니다. live source digest가 달라져도 historical restore는 성공하며 차이는 `rpo_divergence:*`로 기록합니다.

실제 사고 복구는 다음 순서를 지킵니다.

1. API와 모든 worker를 중지해 원본 DB 쓰기를 멈추고 마지막 정상 backup dump와 `.sha256`을 선택합니다.
2. `./ops/restore.sh BACKUP.dump alpha_recovered_YYYYMMDD`로 같은 PostgreSQL 인스턴스의 새 DB에 복원합니다.
3. `sh ./ops/recovery-smoke.sh BACKUP.dump RESTORED_DATABASE [LIVE_SOURCE_DATABASE]`로 선택 dump/checksum/manifest와 restored DB를 직접 검증합니다. live source는 RPO 비교가 필요할 때만 추가합니다. 원본이 손실돼도 historical manifest 검증은 동일하게 수행합니다.
4. `.env`의 `DATABASE_URL` database 이름만 새 DB로 바꾸고 `config --quiet`를 실행합니다.
5. `up -d`를 실행합니다. migration과 publication gate가 모두 성공하기 전에는 트래픽을 전환하지 않습니다.
6. `/health/ready`, `/metrics`, OAuth 로그인, 문제 조회·제출, C++/Python/Java 판정, 수업 tenant 경계를 점검합니다.
7. 롤백 기간 동안 이전 DB를 읽기 전용으로 보존한 뒤 승인된 보존정책에 따라 파기합니다.

전환 뒤 새 백업은 `./ops/backup.sh OUTPUT_DIRECTORY alpha_recovered_YYYYMMDD_unique`처럼 recovered database 이름을 명시합니다. `DATABASE_URL`만 바꾸고 기존 `alpha`를 암묵적으로 백업하는 명령은 사용하지 않습니다.

## 고아 lease

먼저 내부 `/metrics`와 관리자 운영 화면에서 다음 항목을 확인합니다.

```bash
docker compose -f compose.production.yaml exec -T api \
  curl --fail --silent http://127.0.0.1:8080/metrics | \
  grep -E 'alpha_(judge_workers|content_generation_jobs|workspace_runs)'
docker compose -f compose.production.yaml logs --since=30m judge-worker content-worker workspace-worker
```

- judge: 다음 정상 worker polling이 만료 lease를 재대기시키며 최대 시도 초과 건은 `dead`로 종료합니다.
- content generation: 다음 content worker polling이 만료 attempt를 감사 로그에 남기고 재시도 또는 실패 처리합니다.
- workspace: 다음 workspace worker polling이 만료 실행을 다시 lease하며 최대 시도 초과 건은 `expired`로 종료합니다.

worker heartbeat, 고정 실행 이미지 digest, Docker socket과 작업 디렉터리 권한을 복구한 뒤 해당 worker만 재시작합니다. lease token과 감사 추적을 깨뜨리므로 상태·lease 컬럼을 SQL로 직접 수정하지 않습니다. 만료 지표가 worker 재기동 뒤에도 줄지 않으면 관련 로그와 resource ID를 보존하고 배포를 중단합니다. `dead`·`failed`·`expired` 건은 제품의 재채점·재시도·새 실행 흐름으로 다시 요청합니다.

## 사고 기록

request ID, resource ID, 이미지 digest, UTC/KST 시각, 실행 명령, gate/복구 digest와 결과를 남깁니다. 사용자 소스, token, receipt secret, dump 내용은 티켓이나 채팅에 복사하지 않습니다. 보안 사고는 [`SECURITY.md`](SECURITY.md)의 비공개 절차로 처리합니다.

# ALPHA

한국어 우선 온라인 저지 및 소프트웨어 학습 플랫폼입니다.

## 구성

- `src/bin/api.rs`: Rust/Axum API
- `src/bin/metadata_worker.rs`: 외부 문제 메타데이터 주기 갱신 작업자
- `src/bin/judge_worker.rs`: 신뢰성 큐를 소비하는 별도 Docker 판정 작업자
- `judge/Dockerfile`: C++20·Python 3·Java 21 실행 이미지
- `migrations/`: PostgreSQL 스키마
- `OPERATIONS.md`: 배포·복구·고아 lease·게시 게이트 runbook
- `LEGAL_NOTICE.md`: 한국 서비스 공개 전 필수 법적 확인사항
- `THIRD_PARTY_NOTICES.md`: 의존성·컨테이너 고지와 허용 정책
- `web/`: React/Vite 웹

## 로컬 실행

Docker Desktop 또는 Docker Engine과 Compose만 필요합니다.

```bash
cp .env.compose.example .env
docker compose up --build
```

웹은 기본 `http://localhost:8080`에서 열립니다. DB, 마이그레이션, API, 웹, 메타데이터 작업자, 판정 이미지와 판정 작업자가 함께 기동됩니다. 개발용 로그인이 필요하면 `.env`의 `TEST_IDENTITY_ENABLED=true`를 사용합니다. 이 값은 production에서 시작 단계부터 거부됩니다.

생존 상태는 `/health/live`, DB를 포함한 준비 상태는 `/health/ready`, 내부 Prometheus 지표는 `/metrics`입니다. API 요청에는 `x-request-id`가 생성·전파됩니다. OpenAPI 계약은 `openapi.yaml`이며 다음 명령으로 Axum 라우트와의 누락을 확인합니다.

```bash
ruby ops/check-openapi.rb
```

## 운영 배포

```bash
cp .env.production.example .env
# 비밀값과 registry/repository@sha256:... 이미지 참조를 모두 교체
docker compose -f compose.production.yaml config
docker compose -f compose.production.yaml up -d
```

운영 구성은 `DATABASE_URL`, `POSTGRES_PASSWORD`, `SESSION_SECRET`, `WORKSPACE_RECEIPT_SECRET`, `PUBLIC_BASE_URL`, API·웹·판정/작업공간 작업자·실행 이미지와 작업공간 경로가 없으면 실패합니다. 모든 이미지는 `repository@sha256:...` 참조여야 합니다. OAuth를 켤 때는 Google 또는 GitHub의 client id/secret 쌍을 모두 설정해야 하고 `PUBLIC_BASE_URL`은 HTTPS여야 합니다. TLS는 Compose 앞의 신뢰된 리버스 프록시에서 종료합니다.

`docker compose ... up -d`는 마이그레이션 뒤 `publication_gate`를 실행합니다. 현재 리비전에 상업 이용·재배포가 승인되지 않은 콘텐츠가 하나라도 있거나 DB 검증이 실패하면 API와 작업자는 시작하지 않습니다. 운영 절차와 장애 대응은 [`OPERATIONS.md`](OPERATIONS.md)를 따릅니다.

결제는 production에서도 강제로 비활성화됩니다. 콘텐츠 AI는 기본 비활성화이며 provider 계약·권리·개인정보 처리와 credential 주입을 확인한 운영자만 명시적으로 활성화합니다. API와 DB는 호스트에 공개하지 않으며 웹 프록시만 공개합니다. 판정 작업자는 Docker 소켓을 가지므로 전용 호스트에 배치하고 일반 API·웹 노드와 분리해야 합니다.

## 백업과 복구

```bash
./ops/backup.sh
./ops/restore.sh backups/alpha-YYYYMMDDTHHMMSSZ-PID.dump alpha_recovered
./ops/recovery-smoke.sh
```

백업은 PostgreSQL custom format과 SHA-256 체크섬을 함께 생성합니다. 복구는 기존 DB와 `alpha` 운영 DB를 덮어쓰지 않고 새 DB에만 수행합니다. 복구 smoke는 생성 작업, 검토 영수증, 정책 동의, 프로젝트/작업공간 해시를 정렬된 canonical JSON으로 만들고 원본·복원 SHA-256과 원문을 모두 비교합니다. 검증 후 `DATABASE_URL`을 새 DB로 바꾸고 API·작업자를 재기동해 전환합니다. production Compose에서는 배포 디렉터리의 `.env`를 채우고 `COMPOSE_FILE=compose.production.yaml`을 지정해 같은 스크립트를 실행합니다.

공개 운영 전 [`LEGAL_NOTICE.md`](LEGAL_NOTICE.md)의 미확정 운영자 정보를 채우고 화면의 이용약관·개인정보 처리방침과 일치시켜야 합니다. 프로젝트와 제3자 구성요소 고지는 [`LICENSE`](LICENSE), [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)를 확인하세요.

## 외부 기능 상태

- 결제와 AI는 기본값이 `disabled`입니다.
- BOJ·solved.ac 실시간 연동은 2026년 4월 28일 이후 상태를 반영해 `blocked`로 다루며, 확인되지 않은 난이도나 태그를 생성하지 않습니다.

solved.ac 연동은 서버 전용 API v3 어댑터 뒤에 격리되어 있습니다. 레벨 `0`은 미평가로 보존하고, 응답 해시·가져온 시각·어댑터 버전·한국어 태그·신선도 상태를 캐시합니다. 일시 실패 시 마지막 정상 값을 `stale`로 유지하며, 정상 값이 없으면 `blocked`로 표시합니다. 현재는 `SOLVED_AC_ENABLED=false`가 기본값이므로 주기 작업자와 관리자 갱신 모두 외부 요청을 보내지 않습니다. 정책과 접근 허가를 다시 확인한 뒤에만 활성화해야 합니다.

## 성장과 보상

XP는 `xp_events` 원장에만 생성되며 `event_id`와 사용자별 `reward_key`가 각각 고유합니다. 따라서 같은 이벤트 재생과 같은 활동·문제의 반복 제출은 XP를 늘리지 않습니다. 활동 보상은 도움 최고 단계에 따라 독립·도움·리뷰·전체 설명 숙련으로 구분하고, 샘플·사용자 입력 실행은 보상하지 않습니다.

스트릭 날짜는 한국 표준시의 의미 있는 첫 학습만 반영합니다. 기본 보호권 한 개는 하루 공백을 한 번 복구하며, 보상 가능한 활동이 없는 날은 인위적으로 스트릭을 늘릴 수 없습니다. 공개 랭킹은 프로필 공개와 랭킹 표시를 모두 허용한 사용자만 포함합니다. solved.ac 문제 난이도와 ALPHA 사용자 숙련도·XP는 서로 다른 지표입니다.

## 대회 운영

대회는 공개·초대 코드·조직 전용 공개 범위와 ICPC 페널티·최고 점수 방식을 지원합니다. 참가자는 대회 페이지에서 등록한 뒤 대회 문맥으로 제출하며, 대회 정답은 일반 독립 숙련과 분리된 `contest_verified` 근거와 중복 불가능한 XP로 기록됩니다.

ICPC 점수판은 문제별 첫 정답 시각과 그 전의 확정 오답마다 20분을 계산합니다. 프리즈 이후 제출은 종료 전까지 결과 대신 시도 여부만 보이며, 종료 후 `CONTEST_MANAGER` 또는 `ADMIN`이 레이팅을 확정합니다. 확정 작업은 참가자별 기록과 감사 이벤트를 남기고 재실행해도 레이팅을 중복 반영하지 않습니다.

## 조직과 교실

전역 `INSTRUCTOR` 또는 `ADMIN`만 조직을 만들 수 있습니다. 조직 소유자·관리자·강사는 학급과 비공개 문제·활동 과제를 배정하고, 학습자는 일회용·만료형 초대 링크를 수락해 참여합니다. 다른 조직의 학급과 강사 대시보드는 존재 여부를 숨긴 404로 차단합니다.

진도는 배정 이후의 실제 활동 통과와 정식 문제 정답만 집계합니다. 과제를 모두 끝내면 코스 완료 XP와 `instructor_course` 숙련 근거가 원장에 한 번 기록됩니다. 강사 화면은 학급 목표, 학습자별 완료율, 공통 판정 오류를 보여 주며 CSV 내보내기는 스프레드시트 수식 주입 문자를 이스케이프합니다.

## 커뮤니티와 운영

`/community`는 공지·문제 질문·토론과 답변 채택을 제공합니다. 본문은 HTML로 해석하지 않는 일반 텍스트이며, 신고는 한 사용자와 대상마다 열린 건 하나만 허용합니다. `MODERATOR`와 `ADMIN`은 `/admin`에서 신고를 숨김 또는 기각 처리하고 판단 근거와 감사 이벤트를 남깁니다. 공지 작성, 워커 상태, 감사 로그는 `ADMIN`에게만 공개됩니다.

`PROBLEM_SETTER`는 자신이 만든 원본 문제만 개정하거나 재채점할 수 있고 `ADMIN`은 전체 문제를 관리할 수 있습니다. 각 리비전의 공개·비공개 테스트는 리비전에 귀속되어 이미 큐에 들어간 제출의 판정 입력을 바꾸지 않습니다. DB 복합 외래키가 서로 다른 문제와 리비전의 테스트 결합을 거부하며, 콘텐츠 해시는 제목·설명·설정·태그·모든 테스트를 포함합니다. 재채점은 10자 이상의 사유를 요구하고 요청자·대상·재등록 수를 감사 로그에 기록합니다.

## 판정 작업자

판정 작업자는 API 프로세스와 분리해 실행합니다.

```bash
docker build -t alpha-judge-runner:local judge
export JUDGE_IMAGE=alpha-judge-runner:local
cargo run --bin judge_worker
```

실행 컨테이너는 비루트 사용자, 네트워크 없음, 읽기 전용 루트, 제한된 tmpfs, 전체 capability 제거, `no-new-privileges`, PID·CPU·메모리·swap·파일·descriptor·출력 제한을 적용합니다. 숨은 테스트는 컨테이너에 마운트하지 않고 stdin으로만 전달합니다. 운영 환경은 `JUDGE_IMAGE`를 `repository@sha256:...` 형식으로 고정해야 시작됩니다.

Docker 기본 seccomp 외의 커널 격리는 호스트 설정에 의존합니다. 공용 인터넷 판정은 전용 judge 호스트를 사용하고, 위험도에 따라 gVisor·Kata Containers 같은 추가 경계를 적용해야 합니다. Docker 소켓은 작업자 호스트에만 두며 실행 컨테이너에는 절대 전달하지 않습니다.

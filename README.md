# ALPHA

한국어 우선 온라인 저지 및 소프트웨어 학습 플랫폼입니다.

## 구성

- `src/bin/api.rs`: Rust/Axum API
- `src/bin/metadata_worker.rs`: 외부 문제 메타데이터 주기 갱신 작업자
- `src/bin/judge_worker.rs`: 신뢰성 큐를 소비하는 별도 Docker 판정 작업자
- `judge/Dockerfile`: C++20·Python 3·Java 21 실행 이미지
- `migrations/`: PostgreSQL 스키마
- `web/`: React/Vite 웹

## 로컬 실행

Rust 1.97 이상, Node.js 24 이상, PostgreSQL 17이 필요합니다.

```bash
cp .env.example .env
export DATABASE_URL=postgres://alpha:alpha@127.0.0.1:5432/alpha
export SESSION_SECRET=32바이트-이상의-로컬-전용-비밀값으로-교체
cargo run --bin api
```

API는 시작할 때 대기 중인 데이터베이스 마이그레이션을 적용합니다. 생존 상태는 `/health/live`, DB를 포함한 준비 상태는 `/health/ready`에서 확인합니다.

Google·GitHub OAuth는 각 provider의 client id와 secret을 모두 설정해야 활성화됩니다. 운영 OAuth의 `PUBLIC_BASE_URL`은 HTTPS만 허용합니다. `TEST_IDENTITY_ENABLED=true`는 development/test에서만 사용할 수 있고 production 시작 단계에서 거부됩니다.

```bash
cd web
npm ci
npm run dev
```

## 외부 기능 상태

- 결제와 AI는 기본값이 `disabled`입니다.
- BOJ·solved.ac 실시간 연동은 2026년 4월 28일 이후 상태를 반영해 `blocked`로 다루며, 확인되지 않은 난이도나 태그를 생성하지 않습니다.

solved.ac 연동은 서버 전용 API v3 어댑터 뒤에 격리되어 있습니다. 레벨 `0`은 미평가로 보존하고, 응답 해시·가져온 시각·어댑터 버전·한국어 태그·신선도 상태를 캐시합니다. 일시 실패 시 마지막 정상 값을 `stale`로 유지하며, 정상 값이 없으면 `blocked`로 표시합니다. 현재는 `SOLVED_AC_ENABLED=false`가 기본값이므로 주기 작업자와 관리자 갱신 모두 외부 요청을 보내지 않습니다. 정책과 접근 허가를 다시 확인한 뒤에만 활성화해야 합니다.

## 판정 작업자

판정 작업자는 API 프로세스와 분리해 실행합니다.

```bash
docker build -t alpha-judge-runner:local judge
export JUDGE_IMAGE=alpha-judge-runner:local
cargo run --bin judge_worker
```

실행 컨테이너는 비루트 사용자, 네트워크 없음, 읽기 전용 루트, 제한된 tmpfs, 전체 capability 제거, `no-new-privileges`, PID·CPU·메모리·swap·파일·descriptor·출력 제한을 적용합니다. 숨은 테스트는 컨테이너에 마운트하지 않고 stdin으로만 전달합니다. 운영 환경은 `JUDGE_IMAGE`를 `repository@sha256:...` 형식으로 고정해야 시작됩니다.

Docker 기본 seccomp 외의 커널 격리는 호스트 설정에 의존합니다. 공용 인터넷 판정은 전용 judge 호스트를 사용하고, 위험도에 따라 gVisor·Kata Containers 같은 추가 경계를 적용해야 합니다. Docker 소켓은 작업자 호스트에만 두며 실행 컨테이너에는 절대 전달하지 않습니다.

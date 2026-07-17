# ALPHA

한국어 우선 온라인 저지 및 소프트웨어 학습 플랫폼입니다.

## 구성

- `src/bin/api.rs`: Rust/Axum API
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

```bash
cd web
npm ci
npm run dev
```

## 외부 기능 상태

- 결제와 AI는 기본값이 `disabled`입니다.
- BOJ·solved.ac 실시간 연동은 2026년 4월 28일 이후 상태를 반영해 `blocked`로 다루며, 확인되지 않은 난이도나 태그를 생성하지 않습니다.

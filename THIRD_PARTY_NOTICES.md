# Third-party notices

ALPHA 자체 코드는 [`LICENSE`](LICENSE)의 MIT License로 배포됩니다. 아래 구성요소는 각 저작권자와 라이선스 조건을 따릅니다. 이 파일은 제3자 라이선스를 ALPHA 라이선스로 변경하지 않습니다.

## 애플리케이션 의존성

- Rust 직접 의존성: Axum, async-stream, base64, futures-core, getrandom, reqwest, Serde/serde_json, SHA-2, SQLx, subtle, tempfile, thiserror, time, Tokio, tower-http, tracing/tracing-subscriber, UUID, url. 정확한 전이 버전과 checksum은 `Cargo.lock`에 고정됩니다.
- Web 직접 의존성/도구: React, React DOM, Vite, TypeScript, `@vitejs/plugin-react`, Playwright, axe-core, Testing Library, jsdom, Vitest. 정확한 전이 버전, registry URL, integrity와 license는 `web/package-lock.json`에 고정됩니다.

Rust 허용 목록은 `deny.toml`의 Apache-2.0, BSD-3-Clause, CDLA-Permissive-2.0, ISC, MIT, Unicode-3.0, Zlib입니다. Web lockfile 허용 목록은 0BSD, Apache-2.0, BSD-2-Clause, BSD-3-Clause, BlueOak-1.0.0, CC0-1.0, ISC, MIT, MIT-0, MPL-2.0입니다. MPL-2.0 구성요소를 수정·배포하면 해당 파일 수준 source 제공 의무를 별도 검토합니다. dependency review는 GPL-2.0-only/or-later, GPL-3.0-only/or-later와 AGPL-1.0/3.0-only/or-later 신규 의존성을 차단합니다.

## Container 기반 구성요소

- Rust·Node builder images: Rust 프로젝트 및 Node.js 프로젝트의 각 라이선스
- Debian·Ubuntu·Alpine 기반 image와 설치 package: 각 distribution/package의 개별 라이선스
- PostgreSQL image/server: PostgreSQL License
- Docker CLI: Apache License 2.0
- nginx-unprivileged/nginx: image의 Apache-2.0 고지와 nginx의 BSD-2-Clause 조건
- C++ compiler/runtime, Python 3, OpenJDK 21: 각 GCC Runtime Library Exception/GPL 계열 조건, Python Software Foundation License, GPL-2.0-with-Classpath-Exception 및 포함 구성요소 조건
- gitleaks: MIT License

Dockerfile과 운영 Compose의 외부 이미지는 승인 registry와 digest로 제한됩니다. runtime과 web release image는 `LICENSE`, `LEGAL_NOTICE.md`, 이 파일을 `/usr/share/doc/alpha`에 포함합니다. 실제 release workflow는 remote digest 해석, 신뢰 공개키 서명, SLSA provenance attestation과 SPDX JSON SBOM attestation을 모두 확인하며 입력·trust root가 없으면 실패합니다.

## 재현 가능한 확인

```bash
cargo install cargo-audit --version 0.22.2 --locked
cargo audit
cargo install cargo-deny --version 0.20.2 --locked
cargo deny check licenses sources
cd web && npm ci && npm audit --audit-level=high
APPROVED_IMAGE_REGISTRIES=docker.io,ghcr.io ruby ops/check-container-policy.rb
node ops/check-npm-lock.mjs
```

PR CI는 고정 commit의 GitHub Actions, 고정 버전 감사 도구, `Cargo.lock`, 승인 npm registry의 `package-lock.json` URL/integrity, gitleaks digest, 발견된 모든 Dockerfile/운영 image digest를 검사합니다. production image는 `.github/workflows/release-verification.yml`을 실행해 별도로 검증하며 `RELEASE_IMAGE_REFS`, `APPROVED_IMAGE_REGISTRIES`, `COSIGN_PUBLIC_KEY`가 필요합니다. private registry라면 workflow 실행 전에 registry 인증 단계도 제공해야 합니다. 의존성을 추가하거나 올릴 때 이 고지의 구성요소와 허용 정책도 함께 갱신합니다.

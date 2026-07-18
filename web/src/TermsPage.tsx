import { FormEvent, useEffect, useState } from 'react';

const csrfToken = () => document.cookie
  .split(';')
  .map((cookie) => cookie.trim())
  .find((cookie) => cookie.startsWith('alpha_csrf='))
  ?.slice('alpha_csrf='.length) ?? '';

function TermsPage() {
  const [handle, setHandle] = useState('');
  const [agreed, setAgreed] = useState(false);
  const [message, setMessage] = useState('');
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    fetch('/api/v1/auth/me', { credentials: 'include' })
      .then(async (response) => {
        if (response.status === 401) {
          window.location.assign('/login');
          return null;
        }
        if (!response.ok) throw new Error();
        return response.json();
      })
      .then((user) => {
        if (user) setHandle(user.handle);
      })
      .catch(() => setMessage('계정 상태를 불러오지 못했습니다.'));
  }, []);

  const accept = async (event: FormEvent) => {
    event.preventDefault();
    if (!agreed) return;
    setSubmitting(true);
    setMessage('');
    try {
      const response = await fetch('/api/v1/auth/terms', {
        method: 'POST',
        credentials: 'include',
        headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() },
        body: JSON.stringify({ version: '2026-07-18' }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '동의를 저장하지 못했습니다');
      window.location.assign('/onboarding');
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '동의를 저장하지 못했습니다');
      setSubmitting(false);
    }
  };

  return (
    <div className="auth-page">
      <a className="brand auth-brand" href="/" aria-label="ALPHA 홈">ALPHA<span aria-hidden="true">.</span></a>
      <main className="auth-card terms-card" aria-labelledby="terms-title">
        <p className="eyebrow">약관 및 개인정보</p>
        <h1 id="terms-title">시작하기 전,<br /><span>약속을 확인해요</span></h1>
        {handle && <p className="auth-description"><strong>{handle}</strong> 계정의 학습 기록이 어떻게 처리되는지 확인해 주세요.</p>}
        <section className="terms-summary" aria-label="핵심 약관 요약">
          <h2>핵심 원칙</h2>
          <ul>
            <li>제출 소스와 학습 기록은 서비스 제공과 성장 분석에만 사용합니다.</li>
            <li>숨은 테스트, 판정, 랭킹 우위를 판매하지 않습니다.</li>
            <li>결제와 AI는 현재 비활성화되어 있으며 명시적 승인 없이 켜지지 않습니다.</li>
          </ul>
        </section>
        <form onSubmit={accept}>
          <label className="agreement">
            <input type="checkbox" checked={agreed} onChange={(event) => setAgreed(event.target.checked)} />
            <span><strong>이용약관과 개인정보 처리방침을 확인했습니다.</strong><small>버전 2026-07-18 · 전체 문서는 서비스 정책 페이지에서 확인할 수 있습니다.</small></span>
          </label>
          <button className="primary-action terms-submit" type="submit" disabled={!agreed || submitting}>
            {submitting ? '저장 중…' : '동의하고 진단 시작'}
          </button>
        </form>
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default TermsPage;

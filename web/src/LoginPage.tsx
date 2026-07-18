import { FormEvent, useEffect, useState } from 'react';

type Provider = {
  id: 'google' | 'github';
  label: string;
  available: boolean;
  verification: string;
};

type ProviderResponse = {
  providers: Provider[];
  test_identity_available: boolean;
};

const fallbackProviders: Provider[] = [
  { id: 'google', label: 'Google', available: false, verification: 'loading' },
  { id: 'github', label: 'GitHub', available: false, verification: 'loading' },
];

function LoginPage() {
  const [providers, setProviders] = useState(fallbackProviders);
  const [testIdentityAvailable, setTestIdentityAvailable] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [handle, setHandle] = useState('verified-learner');
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState('');
  const redirectAfter = (() => {
    const value = new URLSearchParams(window.location.search).get('redirect_after');
    return value?.startsWith('/') && !value.startsWith('//') ? value : '/';
  })();

  useEffect(() => {
    fetch('/api/v1/auth/providers')
      .then(async (response) => {
        if (!response.ok) throw new Error('provider status unavailable');
        return response.json() as Promise<ProviderResponse>;
      })
      .then((result) => {
        setProviders(result.providers);
        setTestIdentityAvailable(result.test_identity_available);
      })
      .catch(() => setLoadFailed(true));
  }, []);

  const createTestSession = async (event: FormEvent) => {
    event.preventDefault();
    setSubmitting(true);
    setMessage('');
    try {
      const response = await fetch('/api/v1/auth/test-session', {
        method: 'POST',
        credentials: 'include',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ handle }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '로그인하지 못했습니다');
      window.location.assign(body.user?.terms_accepted ? redirectAfter : '/terms');
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '로그인하지 못했습니다');
      setSubmitting(false);
    }
  };

  return (
    <div className="auth-page">
      <a className="brand auth-brand" href="/" aria-label="ALPHA 홈">ALPHA<span aria-hidden="true">.</span></a>
      <main className="auth-card" aria-labelledby="login-title">
        <p className="eyebrow">SECURE SIGN IN</p>
        <h1 id="login-title">배움을 이어갈<br /><span>계정으로 로그인</span></h1>
        <p className="auth-description">소스 코드와 학습 기록을 보호하기 위해 OAuth와 보안 세션을 사용합니다.</p>

        <div className="provider-list" aria-label="OAuth 로그인">
          {providers.map((provider) => provider.available ? (
            <a className="provider-button" href={`/api/v1/auth/${provider.id}/start?redirect_after=${encodeURIComponent(redirectAfter)}`} key={provider.id}>
              {provider.label}로 계속
            </a>
          ) : (
            <button className="provider-button" type="button" disabled key={provider.id} aria-label={`${provider.label} OAuth 설정 필요`}>
              <span>{provider.label}</span><small>운영 자격 증명 필요</small>
            </button>
          ))}
        </div>

        {loadFailed && <p className="auth-message" role="alert">로그인 상태를 불러오지 못했습니다. 잠시 후 다시 시도해 주세요.</p>}

        {testIdentityAvailable && (
          <form className="test-login" onSubmit={createTestSession}>
            <div><strong>개발 검증 모드</strong><span>운영에서는 자동으로 차단됩니다</span></div>
            <label htmlFor="test-handle">개발용 테스트 식별자</label>
            <div className="test-login__controls">
              <input id="test-handle" value={handle} onChange={(event) => setHandle(event.target.value)} required pattern="[a-z0-9](?:[a-z0-9]|-){1,16}[a-z0-9]" />
              <button type="submit" disabled={submitting}>{submitting ? '로그인 중…' : '검증 계정 시작'}</button>
            </div>
          </form>
        )}
        {message && <p className="auth-message" role="alert">{message}</p>}
        <p className="auth-notice">계속하면 약관과 개인정보 처리방침을 확인하고 명시적으로 동의하는 단계로 이동합니다.</p>
      </main>
    </div>
  );
}

export default LoginPage;

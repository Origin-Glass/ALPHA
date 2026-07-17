import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import LoginPage from './LoginPage';

afterEach(() => vi.unstubAllGlobals());

test('외부 OAuth가 차단됐을 때 거짓 로그인 경로를 노출하지 않는다', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({
      providers: [
        { id: 'google', label: 'Google', available: false, verification: 'blocked_missing_credentials' },
        { id: 'github', label: 'GitHub', available: false, verification: 'blocked_missing_credentials' },
      ],
      test_identity_available: true,
    }),
  }));

  render(<LoginPage />);

  await waitFor(() => expect(screen.getByRole('button', { name: 'Google OAuth 설정 필요' })).toBeDisabled());
  expect(screen.getByRole('button', { name: 'GitHub OAuth 설정 필요' })).toBeDisabled();
  expect(screen.getByLabelText('개발용 테스트 식별자')).toBeInTheDocument();
});

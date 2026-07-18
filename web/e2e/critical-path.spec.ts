import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';

const watchBrowserFailures = (page: import('@playwright/test').Page) => {
  const failures: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error' && !message.text().startsWith('Failed to load resource:')) {
      failures.push(`console: ${message.text()}`);
    }
  });
  page.on('pageerror', (error) => failures.push(`page: ${error.message}`));
  page.on('response', (response) => {
    const expectedAbsentDraft = response.status() === 404 && response.url().endsWith('/draft');
    const expectedInvalidatedSession = response.status() === 401
      && response.url().endsWith('/api/v1/auth/me');
    if (response.status() >= 400 && !expectedAbsentDraft && !expectedInvalidatedSession) {
      failures.push(`http: ${response.status()} ${response.url()}`);
    }
  });
  page.on('requestfailed', (request) => {
    const deliberateClose = request.failure()?.errorText.includes('ERR_ABORTED')
      && (request.url().endsWith('/events') || request.url().endsWith('/auth/logout'));
    if (!deliberateClose) {
      failures.push(`network: ${request.method()} ${request.url()} ${request.failure()?.errorText ?? ''}`);
    }
  });
  return failures;
};

test('한국어 문제 탐색은 모바일과 키보드에서 접근 가능하다', async ({ page }) => {
  const browserFailures = watchBrowserFailures(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/problems');

  await expect(page.getByRole('heading', { name: /문제 탐색/ })).toBeVisible();
  await expect(page.getByRole('link', { name: /두 수의 합/ })).toBeVisible();
  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'ALPHA 홈' })).toBeFocused();

  const accessibility = await new AxeBuilder({ page }).analyze();
  const seriousViolations = accessibility.violations.filter(
    (violation) => violation.impact === 'serious' || violation.impact === 'critical',
  );
  expect(seriousViolations).toEqual([]);
  expect(browserFailures).toEqual([]);
});

test('로그인부터 Docker 정답 판정과 로그아웃까지 이어진다', async ({ page }) => {
  const browserFailures = watchBrowserFailures(page);
  const handle = `e2e-${Date.now().toString(36).slice(-8)}`;

  await page.goto('/login');
  await page.getByLabel('개발용 테스트 식별자').fill(handle);
  await page.getByRole('button', { name: '검증 계정 시작' }).click();
  await expect(page).toHaveURL(/\/terms$/);

  await page.getByRole('checkbox', { name: /이용약관과 개인정보 처리방침/ }).check();
  const onboardingLoaded = page.waitForResponse((response) => (
    response.url().endsWith('/api/v1/onboarding') && response.ok()
  ));
  await page.getByRole('button', { name: '동의하고 진단 시작' }).click();
  await expect(page).toHaveURL(/\/onboarding$/);
  await onboardingLoaded;

  await page.goto('/solve/alpha-pair-sum');
  await expect(page.getByRole('heading', { name: '두 수의 합' })).toBeVisible();
  await page.getByRole('textbox', { name: '소스 코드', exact: true }).fill(
    'a, b = map(int, input().split())\nprint(a + b)\n',
  );
  await page.getByRole('button', { name: '정식 제출' }).click();
  await expect(page.getByText('정답', { exact: true })).toBeVisible({ timeout: 45_000 });
  await expect(page.getByText('100점', { exact: true })).toBeVisible();

  await page.goto('/');
  await page.getByRole('button', { name: '로그아웃' }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole('link', { name: '로그인' })).toBeVisible();
  const session = await page.context().request.get('/api/v1/auth/me');
  expect(session.status()).toBe(401);
  expect(browserFailures).toEqual([]);
});

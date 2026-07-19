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

const loginOperator = async (page: import('@playwright/test').Page, redirectAfter: string) => {
  await page.goto(`/login?redirect_after=${encodeURIComponent(redirectAfter)}`);
  await page.getByLabel('개발용 테스트 식별자').fill('e2e-operator');
  await page.getByRole('button', { name: '검증 계정 시작' }).click();
  await expect.poll(() => new URL(page.url()).pathname).toMatch(/^\/(terms|admin|content-studio)$/);
  if (new URL(page.url()).pathname === '/terms') {
    await page.getByRole('checkbox', { name: /이용약관에 동의합니다/ }).check();
    await page.getByRole('checkbox', { name: /개인정보 처리방침에 동의합니다/ }).check();
    const onboardingLoaded = page.waitForResponse((response) => response.url().endsWith('/api/v1/onboarding') && response.ok());
    await page.getByRole('button', { name: '동의하고 진단 시작' }).click();
    await onboardingLoaded;
  }
  if (new URL(page.url()).pathname !== redirectAfter) await page.goto(redirectAfter);
};

const contrastRatio = (locator: import('@playwright/test').Locator) => locator.evaluate((element) => {
  const channel = (value: number) => value / 255 <= .03928 ? value / 255 / 12.92 : ((value / 255 + .055) / 1.055) ** 2.4;
  const luminance = (value: string) => {
    const [red, green, blue] = value.match(/\d+/g)!.slice(0, 3).map(Number).map(channel);
    return .2126 * red + .7152 * green + .0722 * blue;
  };
  const style = getComputedStyle(element);
  const [light, dark] = [luminance(style.color), luminance(style.backgroundColor)].sort((a, b) => b - a);
  return (light + .05) / (dark + .05);
});

const globalOperationIds = {
  learning: '00000000-0000-7000-8000-000000000105',
  workspace: '00000000-0000-7000-8000-000000000106',
};

const operationsWithGlobalItems = {
  providers: { unhealthy: 0, unverified: 0, disabled: 0, action_items: [] },
  generation_jobs: { queued: 0, running: 0, expired_leases: 0, failed: 0, blocked: 0, action_items: [] },
  reviews: { ai_pending: 0, human_pending: 0, rights_pending: 0, pilot_pending: 0, removal_pending: 0, action_items: [] },
  rights: { publication_blockers: 0, action_items: [] },
  learning: { active_projects: 1, stalled_projects: 1, action_items: [{ resource_id: globalOperationIds.learning, state: 'active', reason: 'no_learning_event_7d', age_seconds: 604800 }] },
  workspaces: { queued: 0, running: 0, expired_leases: 0, failed: 1, action_items: [{ resource_id: globalOperationIds.workspace, state: 'failed', reason: 'workspace_failed', age_seconds: 120 }] },
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

test('관리자는 모바일 운영 현황에서 키보드로 조치 화면을 선택할 수 있다', async ({ page }) => {
  const browserFailures = watchBrowserFailures(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route('**/api/v1/admin/operations', (route) => route.fulfill({ json: operationsWithGlobalItems }));
  await loginOperator(page, '/admin');

  const panel = page.locator('.operations-panel');
  await expect(panel.getByRole('heading', { name: '운영 현황' })).toBeVisible();
  const providerAction = panel.getByRole('link', { name: '제공자 관리' });
  const visited: string[] = [];
  for (let index = 0; index < 120 && !visited.includes('생성 작업 확인'); index += 1) {
    await page.keyboard.press('Tab');
    visited.push(await page.locator(':focus').getAttribute('aria-label') ?? await page.locator(':focus').textContent() ?? '');
  }
  expect(visited[0]).toContain('ALPHA');
  expect(visited).toEqual(expect.arrayContaining(['제공자 관리', '생성 작업 확인']));

  const accessibility = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(accessibility.violations).toEqual([]);
  expect(await contrastRatio(providerAction)).toBeGreaterThanOrEqual(4.5);

  const cards = await panel.locator('.operation-section').evaluateAll((elements) => elements.map((element) => {
    const rect = element.getBoundingClientRect();
    return { title: element.querySelector('h3')?.textContent, left: rect.left, top: rect.top };
  }));
  expect(cards.map((card) => card.title)).toEqual(['AI 제공자', '생성 작업', '콘텐츠 검토', '게시 권리', '학습 프로젝트', '작업공간']);
  expect(cards.every((card, index) => index === 0 || (card.left === cards[0].left && card.top > cards[index - 1].top))).toBe(true);

  await panel.getByRole('link', { name: '프로젝트 확인' }).click();
  const learningItem = panel.getByText(globalOperationIds.learning).locator('..');
  await expect(page).toHaveURL(new RegExp(`operation_domain=learning.*resource_id=${globalOperationIds.learning}`));
  await expect(learningItem).toBeFocused();
  await expect(learningItem).toHaveClass(/operation-item-selected/);
  await page.waitForLoadState('networkidle');
  await panel.getByRole('link', { name: '작업공간 확인' }).click();
  const workspaceItem = panel.getByText(globalOperationIds.workspace).locator('..');
  await expect(page).toHaveURL(new RegExp(`operation_domain=workspaces.*resource_id=${globalOperationIds.workspace}`));
  await expect(workspaceItem).toBeFocused();
  await expect(workspaceItem).toHaveClass(/operation-item-selected/);
  await expect(workspaceItem).toHaveAccessibleName(new RegExp(`${globalOperationIds.workspace}.*실행 실패나 임대 만료 상태를 확인하고 담당자에게 실행 식별자를 전달하세요`));
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`operation_domain=learning.*resource_id=${globalOperationIds.learning}`));
  await expect(learningItem).toBeFocused();
  await expect(learningItem).toHaveClass(/operation-item-selected/);
  await page.goForward();
  await expect(page).toHaveURL(new RegExp(`operation_domain=workspaces.*resource_id=${globalOperationIds.workspace}`));
  await expect(workspaceItem).toBeFocused();
  await expect(workspaceItem).toHaveClass(/operation-item-selected/);
  await page.waitForLoadState('networkidle');
  expect(browserFailures).toEqual([]);
});

test('제작자는 모바일 콘텐츠 생성 흐름을 키보드와 명확한 이름으로 사용할 수 있다', async ({ page }) => {
  const browserFailures = watchBrowserFailures(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route('**/api/v1/content/providers', (route) => route.fulfill({ json: { providers: [{ id: 'provider-e2e', name: '로컬 검증 제공자', kind: 'local', protocol: 'openai_compatible', model: 'fixture', cost_per_generation_microunits: 1, liability_cost_per_generation_microunits: 1, enabled: true, credential_available: true }] } }));
  await page.route('**/api/v1/content/jobs', (route) => route.fulfill({ json: { jobs: [] } }));
  await loginOperator(page, '/content-studio');

  await expect(page.getByRole('heading', { name: /제공자는 바꿔도/ })).toBeVisible();
  const provider = page.getByLabel('AI 제공자');
  const contentType = page.getByLabel('콘텐츠 유형');
  const topic = page.getByLabel('생성 주제');
  const count = page.getByLabel('생성 수');
  const submit = page.getByRole('button', { name: '생성 작업 요청' });
  await expect(provider).not.toHaveValue('');
  let firstFocused = '';
  for (let index = 0; index < 40 && !await provider.evaluate((element) => element === document.activeElement); index += 1) {
    await page.keyboard.press('Tab');
    firstFocused ||= await page.locator(':focus').getAttribute('aria-label') ?? await page.locator(':focus').textContent() ?? '';
  }
  expect(firstFocused).toContain('ALPHA');
  await expect(provider).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(contentType).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(topic).toBeFocused();
  await page.keyboard.type('키보드 접근성 생성 주제');
  await page.keyboard.press('Tab');
  await expect(count).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(submit).toBeFocused();
  await expect(submit).toBeEnabled();

  const accessibility = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(accessibility.violations).toEqual([]);
  expect(await contrastRatio(submit)).toBeGreaterThanOrEqual(4.5);

  const sections = await page.locator('.factory-grid > section').evaluateAll((elements) => elements.map((element) => {
    const rect = element.getBoundingClientRect();
    return { heading: element.querySelector('h2')?.textContent, left: rect.left, top: rect.top };
  }));
  expect(sections.map((section) => section.heading)).toEqual(['새 생성 작업', '최근 작업']);
  expect(sections[1].left).toBe(sections[0].left);
  expect(sections[1].top).toBeGreaterThan(sections[0].top);
  expect(browserFailures).toEqual([]);
});

test('로그인부터 Docker 정답 판정과 로그아웃까지 이어진다', async ({ page }) => {
  const browserFailures = watchBrowserFailures(page);
  const handle = `e2e-${Date.now().toString(36).slice(-8)}`;

  await page.goto('/login');
  await page.getByLabel('개발용 테스트 식별자').fill(handle);
  await page.getByRole('button', { name: '검증 계정 시작' }).click();
  await expect(page).toHaveURL(/\/terms$/);

  const acceptButton = page.getByRole('button', { name: '동의하고 진단 시작' });
  await page.getByRole('checkbox', { name: /이용약관에 동의합니다/ }).check();
  await expect(acceptButton).toBeDisabled();
  await page.getByRole('checkbox', { name: /개인정보 처리방침에 동의합니다/ }).check();
  await expect(acceptButton).toBeEnabled();
  const onboardingLoaded = page.waitForResponse((response) => (
    response.url().endsWith('/api/v1/onboarding') && response.ok()
  ));
  await acceptButton.click();
  await expect(page).toHaveURL(/\/onboarding$/);
  await onboardingLoaded;

  await page.goto('/solve/alpha-pair-sum');
  await expect(page.getByRole('heading', { name: '두 수의 합' })).toBeVisible();
  await page.getByRole('textbox', { name: '소스 코드', exact: true }).fill(
    'a, b = map(int, input().split())\nprint(a + b)\n',
  );
  const submissionCreated = page.waitForResponse((response) => (
    response.url().endsWith('/api/v1/submissions')
      && response.request().method() === 'POST'
      && response.status() === 201
  ));
  await page.getByRole('button', { name: '정식 제출' }).click();
  const created = await (await submissionCreated).json() as { id: string };
  await expect.poll(async () => {
    const response = await page.context().request.get(`/api/v1/submissions/${created.id}`);
    const detail = await response.json() as {
      status?: string;
      score?: number | null;
      compile_output?: string | null;
      run_output?: string | null;
    };
    return JSON.stringify({
      http_status: response.status(),
      status: detail.status,
      score: detail.score,
      compile_output: detail.compile_output,
      run_output: detail.run_output,
    });
  }, {
    message: 'Docker 판정 결과가 ACCEPTED 100점이어야 합니다',
    timeout: 45_000,
    intervals: [500, 1_000, 2_000],
  }).toBe(JSON.stringify({
    http_status: 200,
    status: 'ACCEPTED',
    score: 100,
    compile_output: null,
    run_output: null,
  }));
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

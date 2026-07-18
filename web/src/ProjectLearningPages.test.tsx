import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import LearningPlanBuilderPage from './LearningPlanBuilderPage';
import ProjectIdeationPage from './ProjectIdeationPage';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const ok = (body: unknown) => Promise.resolve({ ok: true, status: 201, json: async () => body });

test('AI 비활성 계획도 이유를 보여주고 학습자가 추천을 거부한다', async () => {
  document.cookie = 'alpha_csrf=plan-csrf; path=/';
  const fetchMock = vi.fn()
    .mockReturnValueOnce(ok({ target_outcome: '작동하는 프로젝트', recommendation_key: 'independent-project', reason_codes: ['mastery_gap:independent_coding'], items: [{ kind: 'project_milestone', title: '첫 결과', estimated_minutes: 60 }], provider_used: false, rule_version: 'project-learning-v1' }))
    .mockReturnValueOnce(ok({ rejected: true }));
  vi.stubGlobal('fetch', fetchMock); vi.stubGlobal('crypto', { randomUUID: () => '00000000-0000-7000-8000-000000000001' });
  render(<LearningPlanBuilderPage />); fireEvent.click(screen.getByRole('button', { name: '계획 만들기' }));
  expect(await screen.findByText(/AI 사용 아니요/)).toBeInTheDocument();
  expect(screen.getByText(/mastery_gap:independent_coding/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '이 추천 거부' }));
  await screen.findByText(/추천을 거부했습니다/);
  expect(fetchMock.mock.calls[1][0]).toBe('/api/v1/learning/recommendations/independent-project/reject');
  expect(fetchMock.mock.calls[1][1].headers['x-csrf-token']).toBe('plan-csrf');
});

test('과도한 아이디어의 축소 결과와 첫 보이는 마일스톤을 선택한다', async () => {
  const fetchMock = vi.fn().mockReturnValueOnce(ok({ id: 'idea-1', title: '기록 앱', features: ['기록', '목록'], scope_reduced: true, milestones: [{ position: 1, title: '기록 작동 결과', estimated_minutes: 60 }] }));
  vi.stubGlobal('fetch', fetchMock); vi.stubGlobal('crypto', { randomUUID: () => '00000000-0000-7000-8000-000000000002' });
  render(<ProjectIdeationPage />); fireEvent.click(screen.getByRole('button', { name: '실행 가능한 범위 확인' }));
  expect(await screen.findByText(/5개 이하 기능으로 줄였습니다/)).toBeInTheDocument();
  expect(screen.getByText('60분 · 보이는 결과')).toBeInTheDocument();
  await waitFor(() => expect(JSON.parse(fetchMock.mock.calls[0][1].body).requested_features).toHaveLength(6));
});

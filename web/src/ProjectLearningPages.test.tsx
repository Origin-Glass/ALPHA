import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import LearningPlanBuilderPage from './LearningPlanBuilderPage';
import ProjectIdeationPage from './ProjectIdeationPage';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const ok = (body: unknown) => Promise.resolve({ ok: true, status: 201, json: async () => body });

test('AI 비활성 계획도 이유를 보여주고 학습자가 추천을 거부한다', async () => {
  document.cookie = 'alpha_csrf=plan-csrf; path=/';
  const fetchMock = vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==='/api/v1/learning/plans'&&!options?.method)return ok({revisions:[]});
    if(path==='/api/v1/learning/plans')return ok({id:'p1',revision:1,target_outcome: '작동하는 프로젝트', recommendation_key: 'independent-project', reason_codes: ['mastery_gap:independent_coding'], items: [{ kind: 'project_milestone', title: '첫 결과', estimated_minutes: 60 }], provider_used: false, rule_version: 'project-learning-v1',restored_from_id:null});
    if(path.includes('/reject'))return ok({rejected:true});throw new Error(`unexpected ${path}`);
  });
  let sequence=0;vi.stubGlobal('fetch', fetchMock); vi.stubGlobal('crypto', { randomUUID: () => `key-${++sequence}` });
  render(<LearningPlanBuilderPage />); fireEvent.click(screen.getByRole('button', { name: '계획 만들기' }));
  expect(await screen.findByText(/AI 사용 아니요/)).toBeInTheDocument();
  expect(screen.getByText(/mastery_gap:independent_coding/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '이 추천 거부' }));
  await screen.findByText(/추천을 거부했습니다/);
  const rejection=fetchMock.mock.calls.find(([path])=>String(path).includes('/reject'))!;
  expect(rejection[0]).toBe('/api/v1/learning/recommendations/independent-project/reject');
  expect(rejection[1].headers['x-csrf-token']).toBe('plan-csrf');
  fireEvent.click(screen.getByRole('button',{name:'계획 만들기'}));await waitFor(()=>expect(fetchMock.mock.calls.filter(([path,options])=>path==='/api/v1/learning/plans'&&options?.method==='POST')).toHaveLength(2));
  const planCalls=fetchMock.mock.calls.filter(([path,options])=>path==='/api/v1/learning/plans'&&options?.method==='POST');expect(JSON.parse(planCalls[0][1].body).idempotency_key).not.toBe(JSON.parse(planCalls[1][1].body).idempotency_key);
});

test('과도한 아이디어의 축소 결과와 첫 보이는 마일스톤을 선택한다', async () => {
  const fetchMock = vi.fn().mockImplementation(()=>ok({ id: 'idea-1', title: '기록 앱', features: ['기록', '목록'], scope_reduced: true,feasibility_reasons:['weekly_minutes:120'],excluded_features:[{feature:'채팅',reason_code:'scope_limit'}], milestones: [{ position: 1, title: '기록 작동 결과', estimated_minutes: 60,visible_result:true }] }));
  let sequence=0;vi.stubGlobal('fetch', fetchMock); vi.stubGlobal('crypto', { randomUUID: () => `idea-key-${++sequence}` });
  render(<ProjectIdeationPage />); fireEvent.click(screen.getByRole('button', { name: '실행 가능한 범위 확인' }));
  expect(await screen.findByText(/첫 버전에 필요한 기능으로 줄였습니다/)).toBeInTheDocument();
  expect(screen.getByText(/채팅 제외/)).toBeInTheDocument();
  expect(screen.getByText('60분 · 보이는 결과')).toBeInTheDocument();
  await waitFor(() => expect(JSON.parse(fetchMock.mock.calls[0][1].body).requested_features).toHaveLength(6));
  const firstKey=JSON.parse(fetchMock.mock.calls[0][1].body).idempotency_key;fireEvent.change(screen.getByLabelText(/원하는 기능/),{target:{value:'기록,목록'}});expect(screen.queryByText(/채팅 제외/)).not.toBeInTheDocument();fireEvent.click(screen.getByRole('button',{name:'실행 가능한 범위 확인'}));await waitFor(()=>expect(fetchMock).toHaveBeenCalledTimes(2));expect(JSON.parse(fetchMock.mock.calls[1][1].body).idempotency_key).not.toBe(firstKey);
});

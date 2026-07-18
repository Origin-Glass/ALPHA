import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import LearningPlanBuilderPage from './LearningPlanBuilderPage';
import ProjectIdeationPage from './ProjectIdeationPage';
import ProjectWorkspacePage from './ProjectWorkspacePage';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const ok = (body: unknown) => Promise.resolve({ ok: true, status: 201, json: async () => body });

test('AI 비활성 계획도 이유를 보여주고 학습자가 추천을 거부한다', async () => {
  document.cookie = 'alpha_csrf=plan-csrf; path=/';
  const fetchMock = vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==='/api/v1/learning/plans'&&!options?.method)return ok({revisions:[]});
    if(path==='/api/v1/learning/plans')return ok({id:'p1',revision:1,target_outcome: '작동하는 프로젝트', recommendation_key: 'independent-project', reason_codes: ['mastery_gap:independent_coding'], items: [{ kind: 'project_milestone', title: '첫 결과', estimated_minutes: 60 }], provider_used: false, rule_version: 'project-learning-v1',restored_from_id:null,deadline:'2026-12-31',preferred_framework:'react',privacy:'private'});
    if(path.includes('/reject'))return ok({rejected:true});throw new Error(`unexpected ${path}`);
  });
  let sequence=0;vi.stubGlobal('fetch', fetchMock); vi.stubGlobal('crypto', { randomUUID: () => `key-${++sequence}` });
  render(<LearningPlanBuilderPage />);expect(screen.getByLabelText('알고리즘 추론 진단')).toBeInTheDocument();expect(screen.getByRole('option',{name:'전이 과제'})).toBeInTheDocument();fireEvent.change(screen.getByLabelText('선호 언어'),{target:{value:'python'}});fireEvent.change(screen.getByLabelText('경로 방식'),{target:{value:'exploratory'}});fireEvent.change(screen.getByLabelText('도움 정책'),{target:{value:'transfer_challenge'}});fireEvent.change(screen.getByLabelText('알고리즘 추론 진단'),{target:{value:'73'}}); fireEvent.click(screen.getByRole('button', { name: '계획 만들기' }));
  expect(await screen.findByText(/AI 사용 아니요/)).toBeInTheDocument();
  expect(screen.getByText(/mastery_gap:independent_coding/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '이 추천 거부' }));
  await screen.findByText(/추천을 거부했습니다/);
  const rejection=fetchMock.mock.calls.find(([path])=>String(path).includes('/reject'))!;
  expect(rejection[0]).toBe('/api/v1/learning/recommendations/independent-project/reject');
  expect(rejection[1].headers['x-csrf-token']).toBe('plan-csrf');
  fireEvent.click(screen.getByRole('button',{name:'계획 만들기'}));await waitFor(()=>expect(fetchMock.mock.calls.filter(([path,options])=>path==='/api/v1/learning/plans'&&options?.method==='POST')).toHaveLength(2));
  const planCalls=fetchMock.mock.calls.filter(([path,options])=>path==='/api/v1/learning/plans'&&options?.method==='POST');expect(JSON.parse(planCalls[0][1].body).idempotency_key).not.toBe(JSON.parse(planCalls[1][1].body).idempotency_key);
  expect(JSON.parse(planCalls[0][1].body)).toMatchObject({deadline:'2026-12-31',preferred_framework:'react',origin:'learner',template_id:null,privacy:'private',preferred_language:'python',path_mode:'exploratory',assistance_policy:'transfer_challenge',diagnostic_scores:{algorithmic_reasoning:73}});
});

test('과도한 아이디어의 축소 결과와 첫 보이는 마일스톤을 선택한다', async () => {
  const idea={ id: 'idea-1',revision:1, title: '기록 앱', features: ['학습 기록', '목록'], scope_reduced: true,feasibility_reasons:['weekly_minutes:120'],excluded_features:[{feature:'채팅',reason_code:'scope_limit'}], milestones: [{ position: 1, title: '기록 작동 결과', estimated_minutes: 60,visible_result:true,required_activity_slug:'code-reading',required_skill:'code_literacy' }],source_kind:'original',repository_url:null,repository_revision:null,ownership_basis:null,license_identifier:null,lineage_id:'lineage-1',restored_from_id:null };
  const fetchMock = vi.fn().mockImplementation((path:string,options?:RequestInit)=>path==='/api/v1/projects/ideas'&&!options?.method?ok({ideas:[]}):ok(idea));
  let sequence=0;vi.stubGlobal('fetch', fetchMock); vi.stubGlobal('crypto', { randomUUID: () => `idea-key-${++sequence}` });
  render(<ProjectIdeationPage />);fireEvent.change(screen.getByLabelText('만들고 싶은 이유'),{target:{value:'내 기록을 분석하고 싶음'}});fireEvent.change(screen.getByLabelText('대상 사용자'),{target:{value:'한국 학습자'}});fireEvent.change(screen.getByLabelText('의도한 결과'),{target:{value:'주간 학습 요약'}});fireEvent.change(screen.getByLabelText('핵심 기능'),{target:{value:'주간 요약'}});expect((screen.getByLabelText(/원하는 기능/) as HTMLTextAreaElement).value).toContain('주간 요약');fireEvent.change(screen.getByLabelText('기술'),{target:{value:'python'}});fireEvent.change(screen.getByLabelText('주간 프로젝트 시간(분)'),{target:{value:'240'}});fireEvent.change(screen.getByLabelText('도움 정책'),{target:{value:'independent'}});fireEvent.change(screen.getByLabelText('숙련도'),{target:{value:'advanced'}});fireEvent.change(screen.getByLabelText('실행 환경'),{target:{value:'server'}});fireEvent.change(screen.getByLabelText('인프라'),{target:{value:'container_available'}});fireEvent.change(screen.getByLabelText('소스 종류'),{target:{value:'imported'}});fireEvent.change(screen.getByLabelText('저장소 URL'),{target:{value:'https://github.com/example/owned'}});fireEvent.change(screen.getByLabelText('고정 리비전'),{target:{value:'a'.repeat(40)}});fireEvent.change(screen.getByLabelText('소스 종류'),{target:{value:'original'}});expect(screen.queryByLabelText('저장소 URL')).not.toBeInTheDocument(); fireEvent.click(screen.getByRole('button', { name: '실행 가능한 범위 확인' }));
  expect(await screen.findByText(/첫 버전에 필요한 기능으로 줄였습니다/)).toBeInTheDocument();
  expect(screen.getByText(/채팅 제외/)).toBeInTheDocument();
  expect(screen.getByText(/60분 · 보이는 결과 · 역량 코드 이해 · 활동 code-reading/)).toBeInTheDocument();
  const ideaCalls=()=>fetchMock.mock.calls.filter(([path,options])=>path==='/api/v1/projects/ideas'&&options?.method==='POST');
  await waitFor(() => expect(JSON.parse(ideaCalls()[0][1].body).requested_features).toHaveLength(7));
  expect(JSON.parse(ideaCalls()[0][1].body).requested_features).toContain('학습 기록');
  expect(JSON.parse(ideaCalls()[0][1].body)).toMatchObject({motivation:'내 기록을 분석하고 싶음',target_user:'한국 학습자',intended_outcome:'주간 학습 요약',core_feature:'주간 요약',technology:'python',weekly_minutes:240,assistance_policy:'independent',skill_level:'advanced',runtime:'server',infrastructure:'container_available',source_kind:'original',repository_url:null,repository_revision:null,ownership_basis:null,license_identifier:null});
  const firstKey=JSON.parse(ideaCalls()[0][1].body).idempotency_key;fireEvent.change(screen.getByLabelText(/원하는 기능/),{target:{value:'주간 요약,목록'}});expect(screen.queryByText(/채팅 제외/)).not.toBeInTheDocument();fireEvent.click(screen.getByRole('button',{name:'실행 가능한 범위 확인'}));await waitFor(()=>expect(ideaCalls()).toHaveLength(2));expect(JSON.parse(ideaCalls()[1][1].body).idempotency_key).not.toBe(firstKey);
});

test('핵심 기능을 정규화하고 작업공간 정책을 한국어로 표시한다', async () => {
  const fetchMock=vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==='/api/v1/projects/ideas'&&!options?.method)return ok({ideas:[]});
    if(path==='/api/v1/projects/ideas')return ok({id:'idea-2',revision:1,title:'앱',features:['학습 기록'],scope_reduced:false,feasibility_reasons:[],excluded_features:[],milestones:[{position:1,title:'첫 결과',estimated_minutes:60,visible_result:true,required_activity_slug:'read-code',required_skill:'code_literacy'}],source_kind:'original'});
    if(path==='/api/v1/projects/project-1')return ok({id:'project-1',title:'앱',technology:'typescript',assistance_policy:'documentation_navigator',milestones:[{id:'m1',position:1,title:'첫 결과',estimated_minutes:60,status:'planned',required_skill:'code_literacy',required_activity_slug:'read-code'}]});
    throw new Error(`unexpected ${path}`);
  });
  vi.stubGlobal('fetch',fetchMock);vi.stubGlobal('crypto',{randomUUID:()=> 'stable-key'});
  render(<ProjectIdeationPage />);fireEvent.change(screen.getByLabelText('핵심 기능'),{target:{value:'  학습 기록  '}});fireEvent.click(screen.getByRole('button',{name:'실행 가능한 범위 확인'}));
  await waitFor(()=>expect(fetchMock.mock.calls.some(([path,options])=>path==='/api/v1/projects/ideas'&&options?.method==='POST')).toBe(true));
  const request=fetchMock.mock.calls.find(([path,options])=>path==='/api/v1/projects/ideas'&&options?.method==='POST')!;expect(JSON.parse(request[1].body).core_feature).toBe('학습 기록');
  cleanup();window.history.pushState({},'', '/projects/workspace?id=project-1');render(<ProjectWorkspacePage />);
  expect(await screen.findByText(/도움 정책: 문서 탐색 안내/)).toBeInTheDocument();expect(screen.queryByText(/documentation_navigator/)).not.toBeInTheDocument();
});

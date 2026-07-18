import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import MultiFileWorkspacePage from './MultiFileWorkspacePage';
import UnderstandingPage from './UnderstandingPage';
import PortfolioPage from './PortfolioPage';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); window.history.pushState({}, '', '/'); });
const ok = (body: unknown) => Promise.resolve({ ok: true, status: 200, json: async () => body });

test('여러 파일을 저장하고 격리 실행을 취소하며 같은 저장 재시도에는 같은 멱등성 키를 쓴다', async () => {
  document.cookie = 'alpha_csrf=workspace-csrf; path=/';
  window.history.pushState({}, '', '/workspace?id=w1');
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === '/api/v1/workspaces/w1' && !options?.method) return ok({ id: 'w1', title: '한국어 기록 CLI', version: 1, files: [{ path: 'src/main.rs', content: 'fn main() {}' }, { path: 'README.md', content: '# 기록' }] });
    if (path === '/api/v1/workspaces/w1' && options?.method === 'PUT') return ok({ id: 'w1', title: '한국어 기록 CLI', version: 2, files: JSON.parse(String(options.body)).files });
    if (path === '/api/v1/workspaces/w1/runs') return ok({ id: 'run-1', status: 'running' });
    if (path === '/api/v1/workspace-runs/run-1') return ok({ id: 'run-1', status: 'running', stdout: '', stderr: '' });
    if (path === '/api/v1/workspace-runs/run-1/cancel') return ok({ id: 'run-1', cancel_requested: true });
    throw new Error(`unexpected ${path}`);
  });
  let key = 0;
  vi.stubGlobal('fetch', fetchMock);
  vi.stubGlobal('crypto', { randomUUID: () => `workspace-key-${++key}` });

  render(<MultiFileWorkspacePage />);
  expect(await screen.findByRole('tab', { name: 'src/main.rs' })).toBeInTheDocument();
  fireEvent.click(screen.getByRole('tab', { name: 'README.md' }));
  fireEvent.change(screen.getByLabelText('README.md 내용'), { target: { value: '# 학습 기록' } });
  fireEvent.click(screen.getByRole('button', { name: '파일 저장' }));
  await screen.findByText('파일을 저장했습니다.');
  fireEvent.click(screen.getByRole('button', { name: '파일 저장' }));
  await waitFor(() => expect(fetchMock.mock.calls.filter(([path, options]) => path === '/api/v1/workspaces/w1' && options?.method === 'PUT')).toHaveLength(2));
  const saves = fetchMock.mock.calls.filter(([path, options]) => path === '/api/v1/workspaces/w1' && options?.method === 'PUT');
  expect(JSON.parse(saves[0][1].body).expected_version).toBe(1);
  expect(JSON.parse(saves[1][1].body).expected_version).toBe(2);
  expect(saves[0][1].headers['x-csrf-token']).toBe('workspace-csrf');

  fireEvent.click(screen.getByRole('button', { name: '빌드 및 테스트 실행' }));
  expect(await screen.findByText('실행 중')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '실행 취소' }));
  expect(await screen.findByText('취소됨')).toBeInTheDocument();
});

test('설명·변형·전이 답변을 제출하고 서버 증거만 표시하며 MASTERED로 과장하지 않는다', async () => {
  document.cookie = 'alpha_csrf=understanding-csrf; path=/';
  window.history.pushState({}, '', '/understanding?workspace=w1');
  const fetchMock = vi.fn().mockImplementation((path: string) => {
    if (path === '/api/v1/understanding/challenges') return ok({ id: 'c1', prompt: { explanation: '입력 흐름을 설명하고 다른 자료형에 적용하세요.' }, state: 'REVIEWED' });
    if (path === '/api/v1/understanding/challenges/c1/submit') return ok({ state: 'TRANSFER_VERIFIED', evidence_labels: ['BUILT', 'EXPLAINED', 'INDEPENDENTLY_MODIFIED', 'TRANSFER_VERIFIED'], assistance_disclosure: '공식 문서만 사용' });
    throw new Error(`unexpected ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);
  vi.stubGlobal('crypto', { randomUUID: () => 'understanding-key' });

  render(<UnderstandingPage />);
  expect(await screen.findByText(/입력 흐름을 설명/)).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('코드 목적과 흐름 설명'), { target: { value: '입력을 검증한 뒤 기록 목록에 추가합니다.' } });
  fireEvent.change(screen.getByLabelText('변경 결과 예측'), { target: { value: '빈 입력을 허용하면 빈 기록이 저장됩니다.' } });
  fireEvent.change(screen.getByLabelText('독립 변형 내용'), { target: { value: '검증 함수를 직접 추가했습니다.' } });
  fireEvent.change(screen.getByLabelText('다른 맥락의 전이 답변'), { target: { value: '파일 이름 검증에도 같은 경계 검사를 적용합니다.' } });
  fireEvent.change(screen.getByLabelText('독립 변형 실행 ID'), { target: { value: '00000000-0000-7000-8000-000000000001' } });
  fireEvent.change(screen.getByLabelText('전이 실행 ID'), { target: { value: '00000000-0000-7000-8000-000000000002' } });
  fireEvent.click(screen.getByRole('button', { name: '이해 증거 제출' }));

  expect(await screen.findByRole('heading', { name: '전이 검증됨' })).toBeInTheDocument();
  expect(screen.getByText('도움 공개: 공식 문서만 사용')).toBeInTheDocument();
  expect(screen.getByText('독립적으로 수정함')).toBeInTheDocument();
  expect(screen.queryByText(/MASTERED|마스터|완전 숙달/)).not.toBeInTheDocument();
});

test('포트폴리오는 비공개로 만들고 도움 공개와 검증된 증거 라벨을 게시 전 확인한다', async () => {
  document.cookie = 'alpha_csrf=portfolio-csrf; path=/';
  const item = { id: 'p1', title: '학습 기록 CLI', problem: '기록이 흩어짐', target_user: '한국 학습자', visibility: 'public', assistance_disclosure: '학습자 공개: 질문형 AI 1회 사용', evidence_labels: ['BUILT', 'EXPLAINED'], status: 'draft' };
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === '/api/v1/portfolio' && !options?.method) return ok({ items: [] });
    if (path === '/api/v1/portfolio') return ok(item);
    if (path === '/api/v1/portfolio/p1/publish') return ok({ ...item, status: 'published' });
    throw new Error(`unexpected ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);
  vi.stubGlobal('crypto', { randomUUID: () => 'portfolio-key' });

  render(<PortfolioPage />);
  fireEvent.change(screen.getByLabelText('제목'), { target: { value: '학습 기록 CLI' } });
  fireEvent.change(screen.getByLabelText('해결한 문제'), { target: { value: '기록이 흩어짐' } });
  fireEvent.change(screen.getByLabelText('대상 사용자'), { target: { value: '한국 학습자' } });
  fireEvent.change(screen.getByLabelText('연결할 작업공간 ID'), { target: { value: '00000000-0000-7000-8000-000000000001' } });
  fireEvent.change(screen.getByLabelText('성공한 실행 ID'), { target: { value: '00000000-0000-7000-8000-000000000002' } });
  fireEvent.change(screen.getByLabelText('권리 승인 영수증 ID'), { target: { value: '00000000-0000-7000-8000-000000000003' } });
  expect(screen.getByLabelText('공개 범위')).toHaveValue('private');
  fireEvent.change(screen.getByLabelText('공개 범위'), { target: { value: 'public' } });
  fireEvent.change(screen.getByLabelText('도움 사용 공개'), { target: { value: '질문형 AI 1회 사용' } });
  fireEvent.click(screen.getByRole('button', { name: '포트폴리오 만들기' }));

  expect(await screen.findByText('공개 범위: 공개')).toBeInTheDocument();
  expect(screen.getByText('설명함')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '권리 확인 후 게시' }));
  expect(await screen.findByText('공개 범위: 공개')).toBeInTheDocument();
  const create = fetchMock.mock.calls.find(([path, options]) => path === '/api/v1/portfolio' && options?.method === 'POST')!;
  expect(JSON.parse(create[1].body)).toMatchObject({ visibility: 'public', assistance_disclosure: '질문형 AI 1회 사용', idempotency_key: 'portfolio-key' });
});

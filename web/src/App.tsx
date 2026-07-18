import { useEffect, useState } from 'react';
import './styles.css';
import LoginPage from './LoginPage';
import TermsPage from './TermsPage';
import OnboardingPage from './OnboardingPage';
import LearningPathPage from './LearningPathPage';
import ProblemsPage from './ProblemsPage';
import ProblemDetailPage from './ProblemDetailPage';
import SolvePage from './SolvePage';
import ActivitiesPage from './ActivitiesPage';
import ActivityDetailPage from './ActivityDetailPage';
import DocsTrackPage from './DocsTrackPage';
import ProgressionPage from './ProgressionPage';
import RankingsPage from './RankingsPage';
import PublicProfilePage from './PublicProfilePage';
import ContestsPage from './ContestsPage';
import ContestDetailPage from './ContestDetailPage';
import ClassesPage from './ClassesPage';
import ClassDetailPage from './ClassDetailPage';

const learningAxes = [
  { number: '01', title: '알고리즘 추론', description: '정답보다 사고 과정을 단단하게 만듭니다.' },
  { number: '02', title: '코드 읽기·디버깅', description: '남의 코드를 읽고 오류와 불변식을 찾습니다.' },
  { number: '03', title: '문서 기반 구현', description: '공식 문서를 탐색해 작동하는 결과를 만듭니다.' },
  { number: '04', title: '독립 코딩', description: 'AI 도움 수준을 기록하고 스스로 해결하는 힘을 측정합니다.' },
];

const navigation = [
  { href: '/problems', label: '문제' },
  { href: '/learn', label: '학습' },
  { href: '/contests', label: '대회' },
  { href: '/classes', label: '교실' },
  { href: '/community', label: '커뮤니티' },
  { href: '/rankings', label: '랭킹' },
];

type SessionUser = { handle: string; terms_accepted: boolean };

function SessionControl() {
  const [user, setUser] = useState<SessionUser | null>(null);

  useEffect(() => {
    fetch('/api/v1/auth/me', { credentials: 'include' })
      .then((response) => response.ok ? response.json() : null)
      .then(setUser)
      .catch(() => setUser(null));
  }, []);

  if (!user) return <a className="login-button" href="/login">로그인</a>;

  const logout = async () => {
    const csrf = document.cookie.split(';').map((cookie) => cookie.trim())
      .find((cookie) => cookie.startsWith('alpha_csrf='))?.slice('alpha_csrf='.length) ?? '';
    const response = await fetch('/api/v1/auth/logout', {
      method: 'POST', credentials: 'include', headers: { 'x-csrf-token': csrf },
    });
    if (response.ok) window.location.assign('/');
  };

  return (
    <div className="session-control">
      {!user.terms_accepted && <a href="/terms">약관 확인</a>}
      <span>{user.handle}</span>
      <button type="button" onClick={logout}>로그아웃</button>
    </div>
  );
}

function LandingPage() {

  return (
    <div className="app-shell">
      <header className="site-header">
        <a className="brand" href="/" aria-label="ALPHA 홈">
          ALPHA<span aria-hidden="true">.</span>
        </a>
        <nav className="desktop-nav" aria-label="주요 메뉴">
          {navigation.map((item) => <a href={item.href} key={item.href}>{item.label}</a>)}
        </nav>
        <details className="mobile-menu">
          <summary>메뉴</summary>
          <nav aria-label="모바일 메뉴">
            {navigation.map((item) => <a href={item.href} key={item.href}>{item.label}</a>)}
          </nav>
        </details>
        <SessionControl />
      </header>

      <main>
        <section className="hero" aria-labelledby="hero-title">
          <div className="hero-copy">
            <p className="eyebrow">KOREAN-FIRST SOFTWARE LEARNING</p>
            <h1 id="hero-title" aria-label="생각하는 힘을, 코드로 증명하세요">생각하는 힘을,<br /><span>코드로 증명하세요</span></h1>
            <p className="hero-description">
              빠른 채점과 대회의 긴장감은 유지하고, 코드 독해·디버깅·문서 탐색까지 하나의 성장 경로로 연결합니다.
            </p>
            <div className="hero-actions">
              <a className="primary-action" href="/onboarding">진단 시작하기</a>
              <a className="secondary-action" href="/learn">학습 경로 보기</a>
            </div>
          </div>

          <aside className="today-card" aria-label="오늘의 학습 제안">
            <div className="today-card__top">
              <span>오늘의 경로</span>
              <span className="status-dot">학습 가능</span>
            </div>
            <strong>코드를 먼저 읽어보세요</strong>
            <p>이진 탐색 구현 세 개를 비교하고 경계 조건 오류를 찾습니다.</p>
            <div className="progress" aria-label="오늘 목표 40% 완료">
              <span style={{ width: '40%' }} />
            </div>
            <div className="today-card__meta"><span>예상 18분</span><span>코드 독해 +12 XP</span></div>
          </aside>
        </section>

        <section className="axis-section" aria-labelledby="axis-title">
          <div className="section-heading">
            <p className="eyebrow">ONE PLATFORM, FOUR SKILLS</p>
            <h2 id="axis-title">풀기에서 끝나지 않는 학습</h2>
          </div>
          <div className="axis-grid">
            {learningAxes.map((axis) => (
              <article className="axis-card" key={axis.number}>
                <span>{axis.number}</span>
                <h3>{axis.title}</h3>
                <p>{axis.description}</p>
              </article>
            ))}
          </div>
        </section>

        <section className="service-state" aria-labelledby="service-state-title">
          <div>
            <p className="eyebrow">EXTERNAL METADATA STATUS</p>
            <h2 id="service-state-title">출처와 상태를 숨기지 않습니다</h2>
          </div>
          <p>
            BOJ 서비스와 solved.ac 연동은 2026년 4월 28일 종료됐습니다. ALPHA는 확인된 마지막 메타데이터를 출처와 갱신 시각과 함께 보존하며, 새로운 값을 임의로 만들지 않습니다.
          </p>
          <span className="blocked-badge">LIVE SYNC · BLOCKED</span>
        </section>
      </main>

      <footer><strong>ALPHA</strong><span>한국어 우선 소프트웨어 학습 플랫폼</span></footer>
    </div>
  );
}

function App() {
  if (window.location.pathname === '/login') return <LoginPage />;
  if (window.location.pathname === '/terms') return <TermsPage />;
  if (window.location.pathname === '/onboarding') return <OnboardingPage />;
  if (window.location.pathname === '/learn') return <LearningPathPage />;
  if (window.location.pathname === '/activities') return <ActivitiesPage />;
  if (window.location.pathname.startsWith('/activities/')) return <ActivityDetailPage slug={decodeURIComponent(window.location.pathname.slice('/activities/'.length))} />;
  if (window.location.pathname === '/tracks/docs-builder') return <DocsTrackPage />;
  if (window.location.pathname === '/dashboard') return <ProgressionPage />;
  if (window.location.pathname === '/rankings') return <RankingsPage />;
  if (window.location.pathname.startsWith('/u/')) return <PublicProfilePage handle={decodeURIComponent(window.location.pathname.slice('/u/'.length))} />;
  if (window.location.pathname === '/contests') return <ContestsPage />;
  if (window.location.pathname.startsWith('/contests/')) return <ContestDetailPage slug={decodeURIComponent(window.location.pathname.slice('/contests/'.length))} />;
  if (window.location.pathname === '/classes/join') return <ClassesPage joinMode />;
  if (window.location.pathname === '/classes') return <ClassesPage />;
  if (window.location.pathname.startsWith('/classes/')) return <ClassDetailPage classId={decodeURIComponent(window.location.pathname.slice('/classes/'.length))} />;
  if (window.location.pathname === '/problems') return <ProblemsPage />;
  if (window.location.pathname.startsWith('/problems/')) return <ProblemDetailPage slug={decodeURIComponent(window.location.pathname.slice('/problems/'.length))} />;
  if (window.location.pathname.startsWith('/solve/')) return <SolvePage slug={decodeURIComponent(window.location.pathname.slice('/solve/'.length))} />;
  return <LandingPage />;
}

export default App;

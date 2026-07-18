function PortalHeader() {
  return (
    <header className="portal-header">
      <a className="brand" href="/" aria-label="ALPHA 홈">ALPHA<span aria-hidden="true">.</span></a>
      <nav aria-label="학습 메뉴">
        <a href="/onboarding">역량 진단</a>
        <a href="/learn">나의 경로</a>
        <a href="/learning-plan">계획 만들기</a>
        <a href="/projects/ideas">프로젝트 시작</a>
        <a href="/workspace">실행 작업공간</a>
        <a href="/understanding">이해 검증</a>
        <a href="/portfolio">포트폴리오</a>
        <a href="/activities">읽기·디버깅</a>
        <a href="/tracks/docs-builder">문서 트랙</a>
        <a href="/dashboard">성장</a>
        <a href="/rankings">랭킹</a>
        <a href="/contests">대회</a>
        <a href="/classes">교실</a>
        <a href="/community">커뮤니티</a>
        <a href="/problems">문제 탐색</a>
        <a href="/governance">게시 검토</a>
        <a href="/content-studio">AI 제작실</a>
        <a href="/provider-controls">AI 제공자</a>
        <a href="/policies">정책·개인정보</a>
      </nav>
    </header>
  );
}

export default PortalHeader;

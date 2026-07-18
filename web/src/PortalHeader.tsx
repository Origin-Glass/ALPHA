function PortalHeader() {
  return (
    <header className="portal-header">
      <a className="brand" href="/" aria-label="ALPHA 홈">ALPHA<span aria-hidden="true">.</span></a>
      <nav aria-label="학습 메뉴">
        <a href="/onboarding">역량 진단</a>
        <a href="/learn">나의 경로</a>
        <a href="/activities">읽기·디버깅</a>
        <a href="/tracks/docs-builder">문서 트랙</a>
        <a href="/dashboard">성장</a>
        <a href="/rankings">랭킹</a>
        <a href="/contests">대회</a>
        <a href="/problems">문제 탐색</a>
      </nav>
    </header>
  );
}

export default PortalHeader;

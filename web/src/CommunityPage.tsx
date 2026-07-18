import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Post = {
  id: string;
  kind: "notice" | "question" | "discussion";
  title: string;
  body_preview: string;
  pinned: boolean;
  author_handle: string;
  author_name: string;
  problem_slug: string | null;
  problem_title: string | null;
  answer_count: number;
  has_accepted_answer: boolean;
  created_at: string;
};
type Viewer = { roles: string[] };

const kindLabels = { notice: "공지", question: "질문", discussion: "토론" };
const date = (value: string) =>
  new Intl.DateTimeFormat("ko-KR", {
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    timeZone: "Asia/Seoul",
  }).format(new Date(value));
const csrf = () =>
  document.cookie
    .split(";")
    .map((item) => item.trim())
    .find((item) => item.startsWith("alpha_csrf="))
    ?.slice("alpha_csrf=".length) ?? "";

function CommunityPage() {
  const [items, setItems] = useState<Post[]>([]);
  const [kind, setKind] = useState("");
  const [viewer, setViewer] = useState<Viewer | null>(null);
  const [message, setMessage] = useState("");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [problemSlug, setProblemSlug] = useState("");
  const [postKind, setPostKind] = useState<"question" | "discussion">(
    "question",
  );

  const load = () => {
    const query = kind ? `?kind=${kind}` : "";
    fetch(`/api/v1/community${query}`)
      .then(async (response) => {
        const payload = await response.json();
        if (!response.ok)
          throw new Error(
            payload.error?.message ?? "커뮤니티를 불러오지 못했습니다.",
          );
        return payload;
      })
      .then((payload) => setItems(payload.items))
      .catch((error) =>
        setMessage(
          error instanceof Error
            ? error.message
            : "커뮤니티를 불러오지 못했습니다.",
        ),
      );
  };
  useEffect(load, [kind]);
  useEffect(() => {
    fetch("/api/v1/auth/me", { credentials: "include" })
      .then((response) => (response.ok ? response.json() : null))
      .then(setViewer)
      .catch(() => setViewer(null));
  }, []);

  const create = async (event: FormEvent) => {
    event.preventDefault();
    setMessage("");
    const response = await fetch("/api/v1/community", {
      method: "POST",
      credentials: "include",
      headers: { "content-type": "application/json", "x-csrf-token": csrf() },
      body: JSON.stringify({
        kind: postKind,
        title,
        body,
        problem_slug: problemSlug.trim() || null,
      }),
    });
    const payload = await response.json();
    if (!response.ok) {
      setMessage(payload.error?.message ?? "게시물을 등록하지 못했습니다.");
      return;
    }
    window.location.assign(`/community/${payload.id}`);
  };
  const canOperate = viewer?.roles.some((role) =>
    ["MODERATOR", "PROBLEM_SETTER", "ADMIN"].includes(role),
  );

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="community-page">
        <div className="community-hero">
          <div>
            <p className="eyebrow">ALPHA COMMUNITY</p>
            <h1>
              답보다 먼저,
              <br />
              <span>좋은 질문을</span>
            </h1>
            <p>시도한 방법과 막힌 지점을 나누고, 함께 근거를 찾습니다.</p>
          </div>
          {canOperate && <a href="/admin">운영 콘솔</a>}
        </div>

        <div className="community-filters" role="group" aria-label="게시물 종류">
          {[
            ["", "전체"],
            ["notice", "공지"],
            ["question", "질문"],
            ["discussion", "토론"],
          ].map(([value, label]) => (
            <button
              className={kind === value ? "active" : ""}
              key={value}
              onClick={() => setKind(value)}
              type="button"
            >
              {label}
            </button>
          ))}
        </div>

        <section className="community-list" aria-label="커뮤니티 게시물">
          {items.map((post) => (
            <a href={`/community/${post.id}`} key={post.id}>
              <div className="community-post-meta">
                <span className={`community-kind ${post.kind}`}>
                  {post.pinned && "고정 · "}
                  {kindLabels[post.kind]}
                </span>
                <span>{date(post.created_at)}</span>
              </div>
              <h2>{post.title}</h2>
              <p>{post.body_preview}</p>
              <footer>
                <span>
                  {post.author_name} <small>@{post.author_handle}</small>
                </span>
                <span>
                  {post.problem_title && `# ${post.problem_title} · `}
                  답변 {post.answer_count}개
                  {post.has_accepted_answer && " · 채택 완료"}
                </span>
              </footer>
            </a>
          ))}
          {!items.length && !message && (
            <p className="dashboard-empty">조건에 맞는 게시물이 없습니다.</p>
          )}
        </section>

        <section className="community-compose">
          <div>
            <p className="eyebrow">SHARE YOUR CONTEXT</p>
            <h2>질문 또는 토론 열기</h2>
            <p>정답 코드 대신 재현 조건, 시도한 방법, 예상과 실제의 차이를 적어 주세요.</p>
          </div>
          {viewer ? (
            <form onSubmit={create}>
              <label>
                <span>종류</span>
                <select
                  value={postKind}
                  onChange={(event) =>
                    setPostKind(event.target.value as "question" | "discussion")
                  }
                >
                  <option value="question">질문</option>
                  <option value="discussion">토론</option>
                </select>
              </label>
              <label>
                <span>제목</span>
                <input
                  maxLength={160}
                  value={title}
                  onChange={(event) => setTitle(event.target.value)}
                  required
                />
              </label>
              <label>
                <span>문제 식별자 · 선택</span>
                <input
                  value={problemSlug}
                  onChange={(event) => setProblemSlug(event.target.value)}
                  placeholder="alpha-pair-sum"
                />
              </label>
              <label>
                <span>내용</span>
                <textarea
                  maxLength={10000}
                  value={body}
                  onChange={(event) => setBody(event.target.value)}
                  required
                />
              </label>
              <button type="submit">게시하기</button>
            </form>
          ) : (
            <a className="community-login" href="/login?redirect_after=/community">
              로그인하고 참여하기
            </a>
          )}
        </section>
        {message && (
          <p className="auth-message" role="alert">
            {message}
          </p>
        )}
      </main>
    </div>
  );
}

export default CommunityPage;

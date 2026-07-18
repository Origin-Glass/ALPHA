import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type ClassItem = {
  id: string;
  organization_name: string;
  name: string;
  viewer_role: string;
  learner_count: number;
  active_assignment_count: number;
};
const csrfToken = () =>
  document.cookie
    .split(";")
    .map((cookie) => cookie.trim())
    .find((cookie) => cookie.startsWith("alpha_csrf="))
    ?.slice("alpha_csrf=".length) ?? "";

function ClassesPage({ joinMode = false }: { joinMode?: boolean }) {
  const [items, setItems] = useState<ClassItem[]>([]);
  const [canCreate, setCanCreate] = useState(false);
  const [message, setMessage] = useState("");
  const [code, setCode] = useState(
    new URLSearchParams(window.location.search).get("code") ?? "",
  );
  const [organizationName, setOrganizationName] = useState("");
  const [organizationSlug, setOrganizationSlug] = useState("");
  const [className, setClassName] = useState("");
  useEffect(() => {
    if (joinMode) return;
    fetch("/api/v1/classes", { credentials: "include" })
      .then(async (response) => {
        if (response.status === 401) {
          window.location.assign("/login?redirect_after=/classes");
          return null;
        }
        const body = await response.json();
        if (!response.ok)
          throw new Error(body.error?.message ?? "학급을 불러오지 못했습니다.");
        return body;
      })
      .then((body) => {
        if (body) {
          setItems(body.items);
          setCanCreate(body.can_create);
        }
      })
      .catch((error) =>
        setMessage(
          error instanceof Error
            ? error.message
            : "학급을 불러오지 못했습니다.",
        ),
      );
  }, [joinMode]);
  const accept = async (event: FormEvent) => {
    event.preventDefault();
    setMessage("");
    const response = await fetch("/api/v1/class-invitations/accept", {
      method: "POST",
      credentials: "include",
      headers: {
        "content-type": "application/json",
        "x-csrf-token": csrfToken(),
      },
      body: JSON.stringify({ code }),
    });
    if (response.status === 401) {
      window.location.assign(
        `/login?redirect_after=${encodeURIComponent(`/classes/join?code=${code}`)}`,
      );
      return;
    }
    const body = await response.json();
    if (!response.ok) {
      setMessage(body.error?.message ?? "초대를 수락하지 못했습니다.");
      return;
    }
    window.location.assign(`/classes/${body.class_id}`);
  };
  const create = async (event: FormEvent) => {
    event.preventDefault();
    setMessage("");
    const organization = await fetch("/api/v1/organizations", {
      method: "POST",
      credentials: "include",
      headers: {
        "content-type": "application/json",
        "x-csrf-token": csrfToken(),
      },
      body: JSON.stringify({ slug: organizationSlug, name: organizationName }),
    });
    const organizationBody = await organization.json();
    if (!organization.ok) {
      setMessage(
        organizationBody.error?.message ?? "조직을 만들지 못했습니다.",
      );
      return;
    }
    const classroom = await fetch(
      `/api/v1/organizations/${organizationBody.id}/classes`,
      {
        method: "POST",
        credentials: "include",
        headers: {
          "content-type": "application/json",
          "x-csrf-token": csrfToken(),
        },
        body: JSON.stringify({ name: className }),
      },
    );
    const classBody = await classroom.json();
    if (!classroom.ok) {
      setMessage(classBody.error?.message ?? "학급을 만들지 못했습니다.");
      return;
    }
    window.location.assign(`/classes/${classBody.id}`);
  };
  if (joinMode)
    return (
      <div className="learning-page">
        <PortalHeader />
        <main className="class-join">
          <p className="eyebrow">학급 초대</p>
          <h1>학급 초대 수락</h1>
          <p>강사가 공유한 일회용 초대 코드를 확인합니다.</p>
          <form onSubmit={accept}>
            <label>
              <span>초대 코드</span>
              <input
                value={code}
                onChange={(event) => setCode(event.target.value)}
                required
                autoComplete="off"
              />
            </label>
            <button type="submit">학급에 참여</button>
          </form>
          {message && (
            <p className="auth-message" role="alert">
              {message}
            </p>
          )}
        </main>
      </div>
    );
  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="classes-page">
        <p className="eyebrow">ALPHA 학급</p>
        <div className="classes-heading">
          <div>
            <h1>
              함께 배우는
              <br />
              <span>교실</span>
            </h1>
            <p>과제, 마감, 진행률을 한곳에서 확인합니다.</p>
          </div>
          <a href="/classes/join">초대 코드 입력</a>
        </div>
        <section className="class-list" aria-label="내 학급">
          {items.map((item) => (
            <a href={`/classes/${item.id}`} key={item.id}>
              <div>
                <span>
                  {item.viewer_role === "INSTRUCTOR" ? "강사" : "학습자"}
                </span>
                <small>{item.organization_name}</small>
              </div>
              <h2>{item.name}</h2>
              <p>
                {item.learner_count}명 · 진행 과제{" "}
                {item.active_assignment_count}개
              </p>
            </a>
          ))}
        </section>
        {!items.length && !message && (
          <p className="dashboard-empty">
            참여 중인 학급이 없습니다. 초대 코드를 입력해 시작하세요.
          </p>
        )}
        {canCreate && (
          <section className="class-create">
            <div>
              <p className="eyebrow">강사 설정</p>
              <h2>새 조직과 학급</h2>
            </div>
            <form onSubmit={create}>
              <label>
                <span>조직 이름</span>
                <input
                  value={organizationName}
                  onChange={(event) => setOrganizationName(event.target.value)}
                  required
                />
              </label>
              <label>
                <span>조직 식별자</span>
                <input
                  value={organizationSlug}
                  onChange={(event) =>
                    setOrganizationSlug(event.target.value.toLowerCase())
                  }
                  pattern="[a-z0-9-]{3,48}"
                  placeholder="seoul-alpha-school"
                  required
                />
              </label>
              <label>
                <span>학급 이름</span>
                <input
                  value={className}
                  onChange={(event) => setClassName(event.target.value)}
                  required
                />
              </label>
              <button type="submit">조직·학급 만들기</button>
            </form>
          </section>
        )}
        {message && (
          <p className="auth-message" role="alert">
            {message}
          </p>
        )}
      </main>
    </div>
  );
}

export default ClassesPage;

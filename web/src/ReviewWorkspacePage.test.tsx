import { act,cleanup,fireEvent,render,screen } from "@testing-library/react";
import { afterEach,expect,test,vi } from "vitest";
import ReviewWorkspacePage from "./ReviewWorkspacePage";

afterEach(()=>{cleanup();vi.useRealTimers();vi.unstubAllGlobals();});
const response=(payload:unknown)=>Promise.resolve({ok:true,status:200,json:async()=>payload});

test("검토 대상을 폴링하고 현재 단계에 맞는 사람 승인만 CSRF·멱등키로 보낸다",async()=>{
  vi.useFakeTimers();document.cookie="alpha_csrf=review-csrf; path=/";let loads=0;
  const fetchMock=vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==="/api/v1/content/reviews"){loads++;return response({reviews:[{id:"review-1",revision:1,state:loads>1?"human_review_pending":"ai_review_pending",artifact_hash:"abc",updated_at:"2026-07-19T00:00:00Z"}]});}
    if(path==="/api/v1/content/reviews/review-1"&&!options?.method)return response({id:"review-1",revision:1,state:"human_review_pending",artifact_hash:"abc",artifact:{title:"경계값 검토"},ai_receipts:[
      {kind:"specification_pedagogy",outcome:"pass",provider:"local",model:"m1",prompt_version:"spec-v1",latency_ms:100,usage:{}},
      {kind:"solution_judge",outcome:"pass",provider:"local",model:"m1",prompt_version:"judge-v1",latency_ms:110,usage:{}},
      {kind:"adversarial_rights",outcome:"pass",provider:"local",model:"m1",prompt_version:"rights-v1",latency_ms:120,usage:{}},
    ]});
    if(path==="/api/v1/content/reviews/review-1/human"&&options?.method==="POST")return response({state:"rights_review_pending"});
    throw new Error(`unexpected ${path}`);
  });vi.stubGlobal("fetch",fetchMock);vi.stubGlobal("crypto",{randomUUID:()=>"review-key"});
  render(<ReviewWorkspacePage/>);await act(async()=>{await Promise.resolve();await Promise.resolve();});
  expect(screen.getByText(/외부 AI 검토 실행은 구성되지 않아 차단/)).toBeInTheDocument();
  expect(screen.queryByRole("button",{name:"사람 승인"})).not.toBeInTheDocument();
  await act(async()=>{await vi.advanceTimersByTimeAsync(3000);});fireEvent.click(screen.getByRole("button",{name:"검토 열기"}));await act(async()=>{await Promise.resolve();await Promise.resolve();});
  fireEvent.change(screen.getByLabelText("사람 검토 근거"),{target:{value:"세 검토 증거와 본문 일치 확인"}});fireEvent.click(screen.getByRole("button",{name:"사람 승인"}));
  await act(async()=>{await Promise.resolve();await Promise.resolve();});expect(screen.getByRole("status")).toHaveTextContent("권리 검토 대기");
  const call=fetchMock.mock.calls.find(([path,options])=>path.endsWith("/human")&&options?.method==="POST");expect(call?.[1]?.headers).toEqual({"content-type":"application/json","x-csrf-token":"review-csrf"});expect(JSON.parse(String(call?.[1]?.body)).idempotency_key).toBe("review-key");
});

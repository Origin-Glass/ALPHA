import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Profile = { handle: string; display_name: string; bio: string; title: string | null; cosmetic: string | null; level: number; current_streak: number; independent_mastery_ratio: number; achievements: number };

function PublicProfilePage({ handle }: { handle: string }) {
  const [profile, setProfile] = useState<Profile | null>(null);
  const [message, setMessage] = useState('');
  useEffect(() => { fetch(`/api/v1/profiles/${encodeURIComponent(handle)}`).then(async (response) => { const body = await response.json(); if (!response.ok) throw new Error(body.error?.message ?? '프로필을 볼 수 없습니다.'); return body; }).then(setProfile).catch((error) => setMessage(error instanceof Error ? error.message : '프로필을 볼 수 없습니다.')); }, [handle]);
  return <div className="learning-page"><PortalHeader /><main className="public-profile">{profile ? <section className={`profile-card ${profile.cosmetic ?? ''}`}><p className="eyebrow">PUBLIC ALPHA PROFILE</p><span className="profile-level">LV.{profile.level}</span><h1>{profile.display_name}</h1><p className="profile-handle">@{profile.handle}{profile.title ? ` · ${profile.title}` : ''}</p><p className="profile-bio">{profile.bio || '아직 소개가 없습니다.'}</p><div><article><strong>{profile.current_streak}일</strong><span>현재 스트릭</span></article><article><strong>{Math.round(profile.independent_mastery_ratio * 100)}%</strong><span>독립 숙련</span></article><article><strong>{profile.achievements}</strong><span>업적</span></article></div></section> : !message && <p className="loading-state">프로필을 불러오고 있습니다…</p>}{message && <p className="auth-message" role="alert">{message}</p>}</main></div>;
}

export default PublicProfilePage;

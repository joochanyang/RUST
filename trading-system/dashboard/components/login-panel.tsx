"use client";

import { LogIn, ShieldCheck } from "lucide-react";
import { FormEvent, useState } from "react";

export function LoginPanel() {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const onSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSubmitting(true);
    setError(null);

    try {
      const response = await fetch("/api/session", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ password })
      });

      if (!response.ok) {
        throw new Error("인증에 실패했습니다");
      }

      window.location.reload();
    } catch (loginError) {
      setError(loginError instanceof Error ? loginError.message : "인증에 실패했습니다");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <main className="auth-shell">
      <form className="auth-panel" onSubmit={onSubmit}>
        <div className="auth-icon">
          <ShieldCheck size={22} />
        </div>
        <p className="eyebrow">Trading Operations</p>
        <h1>대시보드 인증</h1>
        <label className="auth-field">
          <span>비밀번호</span>
          <input
            autoFocus
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            autoComplete="current-password"
          />
        </label>
        {error ? <div className="error-strip">{error}</div> : null}
        <button className="auth-submit" type="submit" disabled={submitting || password.length === 0}>
          <LogIn size={16} />
          로그인
        </button>
      </form>
    </main>
  );
}

import type { Metadata } from "next";
import "./styles.css";

export const metadata: Metadata = {
  title: "운영 대시보드",
  description: "페이퍼 및 테스트넷 선물 거래 운영 대시보드"
};

export default function RootLayout({
  children
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="ko">
      <body>{children}</body>
    </html>
  );
}

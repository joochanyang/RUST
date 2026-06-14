import { isDashboardAuthConfigured, isDashboardAuthenticated } from "../lib/auth";
import { LoginPanel } from "../components/login-panel";
import { OpsConsole } from "../components/ops-console";

export const dynamic = "force-dynamic";

export default async function Page() {
  const authRequired = isDashboardAuthConfigured();
  const authenticated = await isDashboardAuthenticated();

  if (!authenticated) {
    return <LoginPanel />;
  }

  return <OpsConsole authRequired={authRequired} />;
}

import { redirect } from "next/navigation";

// There is no landing page and there should not be one yet: everything this site has to say is a
// document, and a second front door would only be a place for a claim to live without a register.
export default function Home() {
  redirect("/docs");
}

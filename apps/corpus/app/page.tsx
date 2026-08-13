import { redirect } from "next/navigation";

// No landing page. Everything this site has to say is a specification, and a second front door is a
// place for a claim to live without a guard behind it.
export default function Home() {
  redirect("/docs");
}

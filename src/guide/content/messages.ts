import type { GuideArticle } from "../types";

export const messages: GuideArticle = {
  id: "messages",
  title: "Messages",
  icon: "message",
  group: "productivity",
  summary: "WhatsApp, Messenger, Microsoft Teams, Telegram and more in one place, each with its own login.",
  open: { module: "messages", label: "Open Messages" },
  needs: "An account with each service you add. Zazabor Home never sees your password: you sign in on the service's own page, inside the panel.",
  topics: [
    {
      title: "Add an account",
      steps: [
        "Open **Messages** on the rail and press **Add account**.",
        "Pick the service (Microsoft Teams asks whether it is a work or school account, or a personal one), and give the account a name.",
        "Press **Add**. The service's own page opens: sign in there. WhatsApp shows a QR code to scan with the phone, as on the web.",
      ],
      tips: [
        "You can add several accounts of one service, for example two WhatsApp numbers, or work and personal Teams. Each keeps its own login.",
        "**Custom web app** takes any https address, for a service that isn't listed.",
        "Zazabor Home only shows the service's official web app. Nothing is read, copied or scripted from your chats.",
      ],
    },
    {
      title: "Work with the accounts",
      steps: [
        "Click an account in the sidebar to show it. Drag it to put the accounts in your order.",
        "Right-click an account for Reload, Zoom, Mute, Hibernate, Open in Browser, Rename and Remove.",
        "**Remove** asks first. Tick **Also sign out and delete its data** to wipe the login and everything the service stored here.",
      ],
      tips: [
        "The number on an account is its unread count; the rail's Messages button adds them up (muted accounts don't count).",
        "A sleeping account has unloaded its page to give memory back. Opening it wakes it, and you are still signed in. **Always on** accounts keep running so their notifications arrive (WhatsApp and Teams start that way); the others sleep after 30 minutes unused, which you can change per account and in Settings → Messages.",
        "Messages shows your phone's texts first in the list when Phone Link is on.",
      ],
      keys: [
        ["Ctrl+1 … 9", "Switch to the account in that place"],
        ["Ctrl+Shift+[", "Previous account"],
        ["Ctrl+Shift+]", "Next account"],
        ["Ctrl+R", "Reload the account"],
        ["Ctrl+K", "Quick switcher: type to find an account"],
      ],
    },
    {
      title: "Notifications and calls",
      steps: [
        "New messages show as desktop notifications with the account's name. Click one to open that account.",
        "Mute one account in its menu, or turn on **Do not disturb** (the bell in the sidebar) to silence them all.",
        "In Settings → Messages, turn **Show previews** off to see only “New message”.",
      ],
      tips: [
        "Voice and video calls can't run inside Zazabor Home, so a call opens in your browser. Sign in to the service there once and calls work.",
        "The camera, microphone and screen are never given to a panel.",
        "Links to other sites open in your default browser. Downloads go to your Downloads folder; program files are refused.",
      ],
    },
  ],
};

# fivesight for Meet

A small Chrome extension that tells the fivesight app on your PC two things:

- **who is speaking** in a Google Meet call, so the transcript says "Budi" instead of "Others";
- **when you join or leave a call**, so fivesight can offer to start and stop recording.

It does not touch audio or video, shows nothing on the Meet page, and only talks to
`127.0.0.1:47285`, the app running on your own PC. Names and timings never go to the internet.

## Install (developer mode)

1. Open `chrome://extensions` and turn on **Developer mode**.
2. Choose **Load unpacked** and pick the extension folder. The app ships a copy: **Settings → Google Meet → Open extension folder**. (Working on fivesight itself? Pick this `extension` folder instead.)
3. In fivesight, open **Settings → Google Meet** and copy the pairing code.
4. Click the extension's icon, paste the code, press **Connect**.

Settings shows "Extension connected" within half a minute, whether or not a Meet tab is open.

## How it works

`content.js` runs on `meet.google.com`. Meet marks each participant tile with a
`data-participant-id`; a speaking person's tile keeps changing as the audio indicator animates, so
`tracker.js` treats sustained activity in a tile as "speaking" (and ignores one-off changes such as
hover). `background.js` posts the events to the app with the pairing code. The app matches them to
transcript lines by time.

## When names stop showing up

Meet changes its markup without notice. Everything that depends on it is in `src/selectors.js`:
the tile selector, how a tile's name is read, and how "in a call" is detected. Turn on
**Debug logging** in the popup and watch the page console for lines starting with `[fivesight]`:
they list the tiles and names it found and each speaking change.

## Tests

```
node --test test/tracker.test.js
```

The speaking and call logic is covered. The page-reading code in `selectors.js` can only be checked
against a real Meet call.

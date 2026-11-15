## Installing

Download [`GameArtGenerator-win-x64-Setup.exe`](https://github.com/SpeakingInBits/GameArtGenerator/releases/download/v{{VERSION}}/GameArtGenerator-win-x64-Setup.exe) and double-click it. The app isn't code-signed yet, so if Windows says "Windows protected your PC", click **More info**, then **Run anyway**. It installs for your Windows account (no administrator password needed) and opens. Next time, open **Game Art Generator** from the Start menu or the desktop.

You need an xAI API key from [console.x.ai](https://console.x.ai). The app asks for it the first time (or uses the `XAI_API_KEY` environment variable if it's set).

After that, the app keeps itself up to date. When a new version is out, it downloads it in the background and installs it the next time you open the app (or right away with **Restart now**).

Your history, settings and images are kept in `%LocalAppData%\SpeakingInBits\GameArtGenerator`, so updating or uninstalling the app doesn't delete them.

If you'd rather not install anything, download [`GameArtGenerator-win-x64-Portable.zip`](https://github.com/SpeakingInBits/GameArtGenerator/releases/download/v{{VERSION}}/GameArtGenerator-win-x64-Portable.zip), right-click it, choose **Extract All**, and double-click `Game Art Generator.exe` in the extracted folder. The portable copy keeps its history and images in a `data` folder inside the extracted folder, and it updates itself too.

The other files in **Assets** (`.nupkg`, `.json`) are what installed apps download to update. You don't need them.

$LINK_WINDOWS = "https://dl.google.com/android/repository/platform-tools-latest-windows.zip"
$LINK_LINUX = "https://dl.google.com/android/repository/platform-tools-latest-linux.zip"
$LINK_MAC = “https://dl.google.com/android/repository/platform-tools-latest-darwin.zip”

Invoke-WebRequest -Uri $LINK_WINDOWS -OutFile "platform-tools-windows.zip"
Expand-Archive -Path "platform-tools-windows.zip" -DestinationPath "platform-tools-windows" -Force
Move-Item -Path "platform-tools-windows\platform-tools\adb.exe" -Destination "adb/win" -Force
Move-Item -Path "platform-tools-windows\platform-tools\AdbWinApi.dll" -Destination "adb/win" -Force
Move-Item -Path "platform-tools-windows\platform-tools\AdbWinUsbApi.dll" -Destination "adb/win" -Force
Remove-Item -Path "platform-tools-windows" -Recurse
Remove-Item -Path "platform-tools-windows.zip"

Invoke-WebRequest -Uri $LINK_LINUX -OutFile "platform-tools-linux.zip"
Expand-Archive -Path "platform-tools-linux.zip" -DestinationPath "platform-tools-linux" -Force
Move-Item -Path "platform-tools-linux\platform-tools\adb" -Destination "adb/linux" -Force
Remove-Item -Path "platform-tools-linux" -Recurse
Remove-Item -Path "platform-tools-linux.zip"

Invoke-WebRequest -Uri $LINK_MAC -OutFile "platform-tools-mac.zip"
Expand-Archive -Path "platform-tools-mac.zip" -DestinationPath "platform-tools-mac" -Force
Move-Item -Path "platform-tools-mac\platform-tools\adb" -Destination "tango-bridge.app/Contents/MacOS" -Force
Remove-Item -Path "platform-tools-mac" -Recurse
Remove-Item -Path "platform-tools-mac.zip"

PackageInfo pInfo = getPackageManager().getPackageInfo(getPackageName(), 0);
String version = pInfo.versionName;

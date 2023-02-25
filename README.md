# Http Drogue

[![Releases](https://img.shields.io/github/v/release/SeriousBug/http-drogue?include_prereleases)](https://github.com/SeriousBug/http-drogue/releases)
[![Docker Image Size](https://img.shields.io/docker/image-size/seriousbug/http-drogue)](https://hub.docker.com/r/seriousbug/http-drogue)
[![MIT license](https://img.shields.io/github/license/SeriousBug/http-drogue)](https://github.com/SeriousBug/http-drogue/blob/master/LICENSE.txt)

Http Drogue is a tiny service that downloads files over HTTP from links you
provide. It can restart and resume interrupted downloads.

![A web page. An input at the top is labelled download URL, with a start download button next to it. A table is located below, with a file being downloaded. The table displays the download speed, remaining time, completed percentage, downloaded size and total size. A refresh list button is above the table.](pub/screenshot.png)

Http Drogue is a lightweight service which is meant to run locally, like a laptop or a home server located where you are. It's best co-located with a service like [Samba SMB server](https://www.samba.org/), [NextCloud](https://nextcloud.com/), or [Bulgur Cloud](https://bulgur-cloud.github.io/) so you can access downloaded files.

# Installation

Use with Docker is the recommended usage method.

You'll need 3 things to run the container:
- A folder to store your downloads. This must be mounted at `/downloads` in the container.
- A folder to store partial download data. This must be mounted at `/data` in the container.
- The environment variable `HTTP_DROGUE_PASSWORD`. You'll use any username with this password to log in.

For example:

```sh
docker run \
  -v $HOME/Downloads:/downloads \
  -v $HOME/.local/share/http-drogue:/data \
  -e HTTP_DROGUE_PASSWORD=correct-horse-battery-staple \
  -p 8080:8080 \
  seriousbug/http-drogue
```

This will store the downloads in `~/Downloads`, and the app data in `~/.local/share`, and use `correct-horse-battery-staple` as the login password. Again, enter any username to log in.
The page will be at `http://localhost:8080`.

Here is a compose file example as well:

```yml
version: '3'

services:
  http-drogue:
    image: seriousbug/http-drogue
    restart: always
    volumes:
      - /path/to/host/downloads:/downloads
      - http-drogue-data:/data
    environment:
      - HTTP_DROGUE_PASSWORD=correct-horse-battery-staple

volumes:
  - http-drogue-data
```

# Usage

Go to `http://localhost:8080`. Enter any username, and the password you picked
in the environment variable. You should see the Http Drogue page.

Paste a URL into the box and hit the button to start download. The file list
below will not update automatically, hit the "Refresh List" button or refresh
the page to update it.

If a download is interrupted, Http Drogue will automatically retry the download.
It can resume the download if the source you are downloading from supports that
as well.

If a download is interrupted over 24 times, Http Drogue will fail the download.
Make sure the URL is correct (you can see the URL by hovering over a file name),
then click the restart button to restart that download.

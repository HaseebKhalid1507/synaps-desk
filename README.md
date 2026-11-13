# Synaps Desktop

Native desktop client for the [Synaps](https://github.com/HaseebKhalid1507/SynapsCLI) daemon.
Windows, macOS and Linux. Built on GPUI (via Longbridge's gpui-kit).

A thin client: it speaks the daemon's wire protocol over the local socket
(Unix domain socket; named pipe on Windows) and never links the engine.

Status: spike.

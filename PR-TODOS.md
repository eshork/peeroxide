# PR TODOs

## Man Pages
- [ ] Fix content
- [ ] Fix naming
- [ ] Add examples

## Changelog
- [ ] peeroxide-cli needs a changelog

## Test All CLIs
- [ ] Send a file via deaddrop
- [X] Send a file via cp via local
- [ ] Send a file via cp via public/remote
- [ ] Test ping
- [ ] Test announce/lookup w/wo data

## node
- [x] should always output bind address/port on startup

stats only show number of peers, but could also show mutable_put buckets and there may be other metrics worth displaying

# cp 
should be able to ctrl+c and exit gracefully, without leaving any hanging processes. This is important for user experience and resource management. Currently does not work during file transfer on recv nor send side. Force killing the process leaves the other side stuck until it is also force killed.


should have a progress bar that can be toggled via flag

send side should have a flag to stay running after transfer completes, to allow for multiple transfers without needing to restart the process. This is a common use case and would improve user experience.

recv side does not detect when the send side has disconnected, and remains stuck until it is force killed. This should be fixed to allow for graceful handling of disconnections.

recv side does not detect arrival of send side if apps started in the wrong order. This should be fixed to allow for more flexible usage.

# deaddrop
pickup side is missing --passphrase flag, so cannot pickup drops that were created with a passphrase


need better integration tests to ensure supported scenarios work as expected (see tests from `cp`), such as:
    Users will expect dd to work in all combinations of these scenarios
    # bootstrap nodes
    - bootstrap nodes are private-local
    - bootstrap nodes are custom-remote
    - bootstrap nodes are public
    # cp send/recv clients
    - are on the same host
    - are on different hosts within same local private network with no firewall
    - on different hosts, same local private network, nat firewall between them
    - are on different hosts across internet, no firewall
    - different internet hosts, one side firewalled
    - different internet hosts, both sides firewalled
    We need unit tests (at the minimum) to ensure these scenarios all work. integration tests where feasible in CI (ie on same host)


# built in defaults
there should be a command to generate a defaults file with sane defaults for all options, so users can easily see what the defaults are and modify them as needed. This would improve user experience and make it easier for users to customize their setup.



Users will expect dd to work in all combinations of these scenarios
# bootstrap nodes
- bootstrap nodes are private-local
- bootstrap nodes are custom-remote
- bootstrap nodes are public
# cp send/recv clients
- are on the same host
- are on different hosts within same local private network with no firewall
- on different hosts, same local private network, nat firewall between them
- are on different hosts across internet, no firewall
- different internet hosts, one side firewalled
- different internet hosts, both sides firewalled
We need unit tests (at the minimum) to ensure these scenarios all work. integration tests where feasible in CI (ie on same host)

So add these to your notes as another requirement for success, in addition to nodejs reference compatibility.
Double check your work with subtasks to verify your work (don't give them full context, so they need to walk the call tree themselves to verify)
If you run into a wedged place where you NEED a --firewalled/--no-firewall/--relay flag from the user to force certain scenario behaviors to verify appropriate operation, adding them is acceptable.

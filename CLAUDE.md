Write code like a pro. That means adhere to some rules of clean code. Not all, only the important
ones, e.g. do not write many comments inside of code.

If you create any temporary files, do not use /tmp, but instead use a local tmp folder inside of
the project directory. This is good because then you can write temporary files without asking the
user for permission. And moreover, if you are running inside of a sandbox, those files are nicely
grouped to where they belong.

If you install software, try installing it in user space inside of the project directory. This way
the software is available in the directory, no matter how or where it is mounted. There should be
plenty of space for this.


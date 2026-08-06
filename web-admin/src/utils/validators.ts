export const validatePhone = (phone: string): boolean => {
  return /^1[3-9]\d{9}$/.test(phone);
};

export const PASSWORD_POLICY_MESSAGE =
  '密码必须为6-20个字符，并同时包含至少一个 ASCII 字母和数字';

export const validatePassword = (password: string): boolean => {
  const characterCount = Array.from(password).length;
  return (
    characterCount >= 6 &&
    characterCount <= 20 &&
    /[A-Za-z]/.test(password) &&
    /[0-9]/.test(password)
  );
};

export const validateNickname = (nickname: string): boolean => {
  return nickname.length >= 1 && nickname.length <= 20;
};
